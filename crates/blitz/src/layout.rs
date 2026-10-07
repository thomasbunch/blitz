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

/// Below this window width at 96 DPI the sidebar collapses to its rail,
/// and from `WIDE` up it expands again; the gap keeps a window dragged
/// across the line from flipping it back and forth.
const NARROW: f32 = 800.0;
const WIDE: f32 = 880.0;

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

    /// Whether the point (`x`, `y`) is inside: the right and bottom edges
    /// are the next rect's.
    pub fn contains(self, x: i32, y: i32) -> bool {
        (self.x..self.right()).contains(&x) && (self.y..self.bottom()).contains(&y)
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

impl Split {
    /// Gives `a` `px` pixels of `area` along the axis, or as near to that
    /// as keeps every pane at least `min`.
    fn set(&mut self, area: Rect, px: i32, min: (i32, i32)) {
        let axis = self.axis;
        let (free, unit) = match axis {
            Axis::Row => (area.w - DIVIDER, min.0),
            Axis::Column => (area.h - DIVIDER, min.1),
        };
        let least = |n: &Node| {
            let k = n.span(axis);
            k * unit + (k - 1) * DIVIDER
        };
        let (lo, hi) = (least(&self.a), free - least(&self.b));
        if free > 0 && lo <= hi {
            self.ratio = px.clamp(lo, hi) as f32 / free as f32;
        }
    }
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

    /// How many panes sit side by side along `axis` on the busiest line
    /// through this node.
    fn span(&self, axis: Axis) -> i32 {
        match self {
            Node::Leaf(_) => 1,
            Node::Split(s) => {
                let (a, b) = (s.a.span(axis), s.b.span(axis));
                if s.axis == axis { a + b } else { a.max(b) }
            }
        }
    }

    /// Moves the divider of the innermost split along `axis` above `p` by
    /// `delta` pixels, stopping where a pane on either side would drop
    /// below `min`. Returns whether there is such a split.
    fn resize(&mut self, p: PaneId, axis: Axis, delta: i32, area: Rect, min: (i32, i32)) -> bool {
        let Node::Split(s) = self else {
            return false;
        };
        let (ra, _, rb) = area.cut(s.axis, s.ratio);
        let inner = if s.a.contains(p) {
            s.a.resize(p, axis, delta, ra, min)
        } else if s.b.contains(p) {
            s.b.resize(p, axis, delta, rb, min)
        } else {
            return false;
        };
        if inner || s.axis != axis {
            return inner;
        }
        let now = match axis {
            Axis::Row => ra.w,
            Axis::Column => ra.h,
        };
        s.set(area, now + delta, min);
        true
    }

    /// Puts each of `p` and `q` where the other was.
    fn swap(&mut self, p: PaneId, q: PaneId) {
        match self {
            Node::Leaf(x) if *x == p => *x = q,
            Node::Leaf(x) if *x == q => *x = p,
            Node::Leaf(_) => {}
            Node::Split(s) => {
                s.a.swap(p, q);
                s.b.swap(p, q);
            }
        }
    }

    fn splits(&self) -> usize {
        match self {
            Node::Leaf(_) => 0,
            Node::Split(s) => 1 + s.a.splits() + s.b.splits(),
        }
    }

    /// Split `n` in the order [`Tab::dividers`] lists their dividers, and
    /// the area it divides.
    fn nth_split(&mut self, n: usize, area: Rect) -> Option<(&mut Split, Rect)> {
        let Node::Split(s) = self else {
            return None;
        };
        let (ra, _, rb) = area.cut(s.axis, s.ratio);
        let k = s.a.splits();
        match n.cmp(&k) {
            std::cmp::Ordering::Less => s.a.nth_split(n, ra),
            std::cmp::Ordering::Equal => Some((&mut **s, area)),
            std::cmp::Ordering::Greater => s.b.nth_split(n - k - 1, rb),
        }
    }

    fn equalize(&mut self) {
        if let Node::Split(s) = self {
            let (a, b) = (s.a.span(s.axis), s.b.span(s.axis));
            s.ratio = a as f32 / (a + b) as f32;
            s.a.equalize();
            s.b.equalize();
        }
    }

    fn walk(&self, area: Rect, panes: &mut Vec<(PaneId, Rect)>, dividers: &mut Vec<(Axis, Rect)>) {
        match self {
            Node::Leaf(p) => panes.push((*p, area)),
            Node::Split(s) => {
                let (a, d, b) = area.cut(s.axis, s.ratio);
                s.a.walk(a, panes, dividers);
                dividers.push((s.axis, d));
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
        dividers.into_iter().map(|d| d.1).collect()
    }

    /// The divider within `slop` pixels of (x, y), as its index in
    /// [`Tab::dividers`] and its split's axis.
    pub fn divider_at(&self, area: Rect, x: i32, y: i32, slop: i32) -> Option<(usize, Axis)> {
        if self.zoom.is_some() {
            return None;
        }
        let mut dividers = Vec::new();
        self.root.walk(area, &mut Vec::new(), &mut dividers);
        dividers.into_iter().enumerate().find_map(|(i, (axis, d))| {
            let (sx, sy) = match axis {
                Axis::Row => (slop, 0),
                Axis::Column => (0, slop),
            };
            let hit =
                (d.x - sx..d.right() + sx).contains(&x) && (d.y - sy..d.bottom() + sy).contains(&y);
            hit.then_some((i, axis))
        })
    }

    /// Moves divider `i` of [`Tab::dividers`] to (x, y), along its axis
    /// and as far as keeps every pane at least `min`. Returns false if
    /// there is no such divider.
    pub fn drag(&mut self, i: usize, x: i32, y: i32, area: Rect, min: (i32, i32)) -> bool {
        let Some((s, r)) = self.root.nth_split(i, area) else {
            return false;
        };
        let px = match s.axis {
            Axis::Row => x - r.x,
            Axis::Column => y - r.y,
        };
        s.set(r, px, min);
        true
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
        let Some(p) = self.neighbour(dir, area) else {
            return false;
        };
        self.focus(p);
        true
    }

    /// Swaps the focused pane with the one [`Tab::focus_dir`] would pick,
    /// moving it a place toward `dir`. It keeps focus. Returns whether it
    /// moved.
    pub fn swap(&mut self, dir: Dir, area: Rect) -> bool {
        let Some(q) = self.neighbour(dir, area) else {
            return false;
        };
        self.root.swap(self.focus, q);
        self.zoom = None;
        true
    }

    fn neighbour(&self, dir: Dir, area: Rect) -> Option<PaneId> {
        let tiles = self.tiles(area);
        let &(_, f) = tiles.iter().find(|t| t.0 == self.focus)?;
        tiles
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
            .min()
            .map(|(.., p)| p)
    }

    /// Moves the nearest divider above the focused pane that runs across
    /// `dir`, by `px` pixels toward `dir`. No pane is squeezed below `min`.
    /// Returns false if no split above the focused pane has that axis.
    pub fn resize(&mut self, dir: Dir, px: i32, area: Rect, min: (i32, i32)) -> bool {
        let delta = match dir {
            Dir::Right | Dir::Down => px,
            Dir::Left | Dir::Up => -px,
        };
        let found = self.root.resize(self.focus, dir.axis(), delta, area, min);
        if found {
            self.zoom = None;
        }
        found
    }

    /// Shows only the focused pane, filling the tab, or ends the zoom.
    /// Focusing another pane, splitting and resizing also end it.
    pub fn toggle_zoom(&mut self) {
        self.zoom = match self.zoom {
            Some(_) => None,
            None => Some(self.focus),
        };
    }

    /// Gives panes equal space. Each split is weighted by how many panes
    /// sit side by side in each half, so three columns made by two splits
    /// come out at a third each rather than a half and two quarters.
    pub fn equalize(&mut self) {
        self.root.equalize();
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
    /// Set while the window is too narrow for the sidebar, which it
    /// collapsed: true when the sidebar expands again once there is room.
    pub narrow: Option<bool>,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            sidebar_expanded: true,
            narrow: None,
        }
    }
}

impl Window {
    /// Whether the sidebar or its rail shows: there are two sessions or
    /// more.
    pub fn has_sidebar(&self) -> bool {
        self.tabs.iter().map(|t| t.panes().len()).sum::<usize>() >= 2
    }

    /// Expands or collapses the sidebar. Returns false when there is none
    /// to change, so the key goes to the program. The user's choice stands
    /// when the window gets wide again.
    pub fn toggle_sidebar(&mut self) -> bool {
        if !self.has_sidebar() {
            return false;
        }
        self.sidebar_expanded = !self.sidebar_expanded;
        if let Some(back) = &mut self.narrow {
            *back = false;
        }
        true
    }

    /// Collapses the sidebar when the window gets narrower than `NARROW`
    /// px at 96 DPI, and expands it again once it is `WIDE`, unless the
    /// user chose otherwise in between. Returns whether the sidebar changed.
    pub fn fit_width(&mut self, width: f32) -> bool {
        let was = self.sidebar_expanded;
        match self.narrow {
            None if width < NARROW => {
                self.narrow = Some(self.sidebar_expanded);
                self.sidebar_expanded = false;
            }
            Some(back) if width >= WIDE => {
                self.narrow = None;
                self.sidebar_expanded |= back;
            }
            _ => {}
        }
        self.sidebar_expanded != was
    }

    /// Whether the sidebar is expanded by the user's choice, which is what
    /// a session saves: a narrow window's collapse is not.
    pub fn chosen_expanded(&self) -> bool {
        self.sidebar_expanded || self.narrow == Some(true)
    }

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

    /// Puts pane `new` where `p` is, with its focus, zoom and place in the
    /// focus history. Returns false if no tab has `p`.
    pub fn replace_pane(&mut self, p: PaneId, new: PaneId) -> bool {
        let Some(t) = self.tabs.iter_mut().find(|t| t.root.contains(p)) else {
            return false;
        };
        if let Some(leaf) = t.root.leaf_mut(p) {
            *leaf = Node::Leaf(new);
        }
        let ids = (t.mru.iter_mut())
            .chain([&mut t.focus])
            .chain(t.zoom.as_mut());
        for q in ids.filter(|q| **q == p) {
            *q = new;
        }
        true
    }
    /// Moves the active tab `by` places, stopping at either end. Returns
    /// false when it is already there.
    pub fn move_tab(&mut self, by: isize) -> bool {
        let last = self.tabs.len().saturating_sub(1);
        let to = self.active.saturating_add_signed(by).min(last);
        if to == self.active {
            return false;
        }
        let t = self.tabs.remove(self.active);
        self.tabs.insert(to, t);
        self.active = to;
        true
    }

    /// Takes pane `p` out of its tab into a new tab named `name` after the
    /// others, and shows that. Returns false when `p` is its tab's only
    /// pane, or in no tab.
    pub fn pane_to_new_tab(&mut self, p: PaneId, name: String) -> bool {
        if !self
            .tabs
            .iter_mut()
            .any(|t| t.root.contains(p) && t.close(p))
        {
            return false;
        }
        self.tabs.push(Tab::new(name, p));
        self.active = self.tabs.len() - 1;
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
    fn rects_hold_points_up_to_their_right_and_bottom_edges() {
        let a = r(10, 20, 5, 4);
        assert!(a.contains(10, 20) && a.contains(14, 23));
        assert!(!a.contains(15, 20) && !a.contains(10, 24) && !a.contains(9, 21));
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
    fn resize_moves_the_nearest_matching_divider() {
        let mut t = four();
        // Above 4, the nearest row split is 3 | 4; the root is untouched.
        assert!(t.resize(Dir::Left, 10, AREA, MIN));
        let rects = t.rects(AREA);
        assert_eq!(rects[0].1, r(0, 0, 500, 601));
        assert_eq!(rects[2].1, r(501, 301, 240, 300));
        assert_eq!(rects[3].1, r(742, 301, 259, 300));
        // The nearest column split is 2 over (3 | 4).
        assert!(t.resize(Dir::Up, 100, AREA, MIN));
        assert_eq!(t.rects(AREA)[1].1, r(501, 0, 500, 200));
        // From 2, the nearest row split is the root.
        t.focus(PaneId(2));
        assert!(t.resize(Dir::Right, 50, AREA, MIN));
        assert_eq!(t.rects(AREA)[0].1, r(0, 0, 550, 601));
    }

    #[test]
    fn resize_keeps_every_pane_above_the_minimum() {
        let mut t = four();
        t.focus(PaneId(1));
        // 3 and 4 sit side by side on the right, which therefore needs
        // 80 + 1 + 80 px.
        assert!(t.resize(Dir::Right, 10_000, AREA, MIN));
        let rects = t.rects(AREA);
        assert_eq!(rects[0].1.w, 1000 - 161);
        assert_eq!((rects[2].1.w, rects[3].1.w), (80, 80));
        assert!(t.resize(Dir::Left, 10_000, AREA, MIN));
        assert_eq!(t.rects(AREA)[0].1.w, 80);
        // Nothing is stacked above or below pane 1.
        let before = t.clone();
        assert!(!t.resize(Dir::Up, 10, AREA, MIN));
        assert_eq!(t, before);
    }

    #[test]
    fn swap_trades_places_with_the_neighbour() {
        let mut t = four();
        let (r3, r2) = (t.rects(AREA)[2].1, t.rects(AREA)[1].1);
        assert!(t.swap(Dir::Left, AREA));
        assert_eq!(t.panes(), ids(&[1, 2, 4, 3]));
        assert_eq!(t.focus, PaneId(4));
        assert_eq!(t.rects(AREA)[2], (PaneId(4), r3));
        assert!(t.swap(Dir::Up, AREA));
        assert_eq!(t.panes(), ids(&[1, 4, 2, 3]));
        assert_eq!(t.rects(AREA)[1], (PaneId(4), r2));
        assert_eq!(t.mru, ids(&[4, 3, 2, 1]), "swapping is not focusing");
        // Nothing is above the top.
        let before = t.clone();
        assert!(!t.swap(Dir::Up, AREA));
        assert_eq!(t, before);
    }

    #[test]
    fn dividers_are_found_and_dragged() {
        let mut t = four();
        // Dividers: the root at x 500, 2 over (3 | 4) at y 300, 3 | 4 at
        // x 751.
        assert_eq!(t.divider_at(AREA, 503, 100, 3), Some((0, Axis::Row)));
        assert_eq!(t.divider_at(AREA, 600, 297, 3), Some((1, Axis::Column)));
        assert_eq!(t.divider_at(AREA, 751, 400, 3), Some((2, Axis::Row)));
        assert_eq!(t.divider_at(AREA, 504, 100, 3), None);
        assert_eq!(t.divider_at(AREA, 751, 100, 3), None, "2 has no divider");

        assert!(t.drag(0, 300, 0, AREA, MIN));
        assert_eq!(t.rects(AREA)[0].1, r(0, 0, 300, 601));
        assert!(t.drag(1, 0, 100, AREA, MIN));
        assert_eq!(t.rects(AREA)[1].1, r(301, 0, 700, 100));
        // 4 stops at the minimum width.
        assert!(t.drag(2, 10_000, 0, AREA, MIN));
        assert_eq!(t.rects(AREA)[3].1.w, 80);
        assert!(!t.drag(3, 0, 0, AREA, MIN));

        t.focus(PaneId(1));
        t.toggle_zoom();
        assert_eq!(t.divider_at(AREA, 300, 100, 3), None);
    }

    #[test]
    fn drag_reaches_splits_nested_on_the_left() {
        // (1 | 2) over 3: the root's first half is a split, so divider 0
        // is inside it and the root is divider 1.
        let mut t = Tab::new("t".into(), PaneId(1));
        assert!(t.split(Dir::Down, PaneId(3), AREA, MIN));
        t.focus(PaneId(1));
        assert!(t.split(Dir::Right, PaneId(2), AREA, MIN));
        assert_eq!(t.panes(), ids(&[1, 2, 3]));
        assert_eq!(
            t.dividers(AREA),
            vec![r(500, 0, 1, 300), r(0, 300, 1001, 1)]
        );
        assert_eq!(t.divider_at(AREA, 500, 100, 3), Some((0, Axis::Row)));
        assert_eq!(t.divider_at(AREA, 100, 300, 3), Some((1, Axis::Column)));
        assert!(t.drag(0, 200, 0, AREA, MIN));
        assert_eq!(t.rects(AREA)[0].1, r(0, 0, 200, 300));
        assert_eq!(t.rects(AREA)[2].1, r(0, 301, 1001, 300), "3 is untouched");
        assert!(t.drag(1, 0, 400, AREA, MIN));
        assert_eq!(t.rects(AREA)[2].1, r(0, 401, 1001, 200));
        assert_eq!(t.rects(AREA)[0].1, r(0, 0, 200, 400), "1 follows the root");
        // Deeper: ((1 | 4) | 2) over 3 puts the new split first.
        t.focus(PaneId(1));
        assert!(t.split(Dir::Right, PaneId(4), AREA, MIN));
        assert_eq!(t.dividers(AREA).len(), 3);
        assert_eq!(t.divider_at(AREA, 100, 100, 3), Some((0, Axis::Row)));
        assert!(t.drag(0, 90, 0, AREA, MIN));
        assert_eq!(t.rects(AREA)[0].1.w, 90);
        // 4 keeps its minimum inside the 200 px the root's left half has.
        assert!(t.drag(0, 150, 0, AREA, MIN));
        assert_eq!(t.rects(AREA)[0].1.w, 200 - 1 - 80);
    }

    #[test]
    fn divider_hits_follow_the_list_order_where_they_meet() {
        let t = four();
        // Where the root divider meets the one under pane 2, both are in
        // reach: the first listed wins.
        assert_eq!(t.divider_at(AREA, 501, 299, 3), Some((0, Axis::Row)));
        assert_eq!(t.divider_at(AREA, 504, 300, 3), Some((1, Axis::Column)));
        // Exactly at the slop's edge, and past it.
        assert_eq!(t.divider_at(AREA, 497, 100, 3), Some((0, Axis::Row)));
        assert_eq!(t.divider_at(AREA, 496, 100, 3), None);
        assert_eq!(t.divider_at(AREA, 500, 100, 0), Some((0, Axis::Row)));
        assert_eq!(t.divider_at(AREA, 501, 100, 0), None, "no slop");
    }

    #[test]
    fn drag_edges() {
        let mut t = four();
        // Past the left edge stops at the minimum.
        assert!(t.drag(0, -500, 0, AREA, MIN));
        assert_eq!(t.rects(AREA)[0].1.w, 80);
        // An area too small for the minimum leaves the ratio alone but
        // still finds the divider.
        let before = t.clone();
        assert!(t.drag(0, 10, 0, r(0, 0, 100, 100), MIN));
        assert_eq!(t, before);
        // Zero-size areas do not divide by zero.
        assert!(t.drag(0, 10, 0, r(0, 0, 0, 0), MIN));
        assert_eq!(t, before);
        assert!(!t.drag(usize::MAX, 0, 0, AREA, MIN));
        // A lone pane has no divider.
        let mut lone = Tab::new("t".into(), PaneId(1));
        assert!(!lone.drag(0, 10, 10, AREA, MIN));
        assert_eq!(lone.divider_at(AREA, 0, 0, 1000), None);
    }

    #[test]
    fn split_at_exactly_the_minimum() {
        // 80 + 1 + 80 fits; 80 + 1 + 79 does not.
        let mut t = Tab::new("t".into(), PaneId(1));
        assert!(t.split(Dir::Right, PaneId(2), r(0, 0, 161, 100), MIN));
        let mut t = Tab::new("t".into(), PaneId(1));
        assert!(!t.split(Dir::Right, PaneId(2), r(0, 0, 160, 100), MIN));
        assert!(
            t.split(Dir::Down, PaneId(2), r(0, 0, 160, 97), MIN),
            "48 + 1 + 48"
        );
        let mut t = Tab::new("t".into(), PaneId(1));
        assert!(!t.split(Dir::Down, PaneId(2), r(0, 0, 160, 96), MIN));
        // A focus that is not in the tab cannot be split.
        t.focus = PaneId(9);
        assert!(!t.split(Dir::Right, PaneId(2), AREA, MIN));
    }

    #[test]
    fn close_after_a_restore_focuses_the_first_pane_of_the_sibling() {
        // A restored tab knows only its focused pane as recently used.
        let mut t = four();
        t.mru = vec![PaneId(1)];
        t.focus = PaneId(1);
        assert!(t.close(PaneId(1)));
        assert_eq!(t.focus, PaneId(2));
        assert_eq!(t.mru, ids(&[2]));
        // Closing a pane that does not have focus keeps focus.
        assert!(t.close(PaneId(4)));
        assert_eq!(t.focus, PaneId(2));
    }

    #[test]
    fn closing_tabs_around_the_active_one() {
        let tabs = |n: u32| {
            let mut w = Window::default();
            for i in 1..=n {
                w.tabs.push(Tab::new(format!("t{i}"), PaneId(i)));
            }
            w
        };
        // The active tab in the middle closes: the one to its right takes
        // its place.
        let mut w = tabs(3);
        w.active = 1;
        assert!(w.close_pane(PaneId(2)));
        assert_eq!(w.tabs[w.active].name, "t3");
        // A tab after the active one closes: the active one stays.
        let mut w = tabs(3);
        assert!(w.close_pane(PaneId(3)));
        assert_eq!((w.active, w.tabs[w.active].name.as_str()), (0, "t1"));
        // The only tab.
        let mut w = tabs(1);
        assert!(w.close_pane(PaneId(1)));
        assert!(w.tabs.is_empty());
    }

    #[test]
    fn zoom_fills_the_tab_until_focus_moves() {
        let mut t = four();
        t.focus(PaneId(3));
        t.toggle_zoom();
        assert_eq!(t.rects(AREA), vec![(PaneId(3), AREA)]);
        assert!(t.dividers(AREA).is_empty());
        t.toggle_zoom();
        assert_eq!(t.rects(AREA), four().rects(AREA));

        t.toggle_zoom();
        assert!(t.focus_dir(Dir::Right, AREA));
        assert_eq!((t.focus, t.zoom), (PaneId(4), None));
        t.toggle_zoom();
        assert!(t.split(Dir::Down, PaneId(5), AREA, MIN));
        assert_eq!(t.zoom, None);
        t.toggle_zoom();
        assert!(t.resize(Dir::Up, 1, AREA, MIN));
        assert_eq!(t.zoom, None);
        t.toggle_zoom();
        assert!(t.close(PaneId(5)));
        assert_eq!((t.focus, t.zoom), (PaneId(4), None));
    }

    #[test]
    fn equalize_weights_by_panes_along_the_axis() {
        let mut t = four();
        t.resize(Dir::Left, 70, AREA, MIN);
        t.focus(PaneId(1));
        t.resize(Dir::Right, 123, AREA, MIN);
        t.equalize();
        // 1, 3 and 4 are three columns of a third each; 2 spans 3 and 4.
        assert_eq!(
            t.rects(AREA),
            vec![
                (PaneId(1), r(0, 0, 333, 601)),
                (PaneId(2), r(334, 0, 667, 300)),
                (PaneId(3), r(334, 301, 333, 300)),
                (PaneId(4), r(668, 301, 333, 300)),
            ]
        );
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

    #[test]
    fn a_replaced_pane_keeps_its_place() {
        let mut w = Window::default();
        w.tabs.push(Tab::new("a".into(), PaneId(9)));
        w.tabs.push(four());
        let before = w.tabs[1].rects(AREA);
        w.tabs[1].toggle_zoom();
        assert!(w.replace_pane(PaneId(4), PaneId(7)));
        let t = &w.tabs[1];
        assert_eq!(t.panes(), ids(&[1, 2, 3, 7]));
        assert_eq!((t.focus, t.zoom), (PaneId(7), Some(PaneId(7))));
        assert_eq!(t.mru[0], PaneId(7));
        assert!(!t.mru.contains(&PaneId(4)));
        w.tabs[1].toggle_zoom();
        let moved = before
            .iter()
            .map(|&(p, r)| (if p == PaneId(4) { PaneId(7) } else { p }, r));
        assert_eq!(w.tabs[1].rects(AREA), moved.collect::<Vec<_>>());
        assert!(!w.replace_pane(PaneId(4), PaneId(8)));
        assert_eq!(w.tabs[0], Tab::new("a".into(), PaneId(9)));
    }

    #[test]
    fn the_sidebar_key_needs_a_sidebar() {
        let mut w = Window::default();
        w.tabs.push(Tab::new("t".into(), PaneId(1)));
        // One session: no sidebar, so the key is the program's.
        assert!(!w.toggle_sidebar());
        assert!(w.sidebar_expanded);
        assert!(w.tabs[0].split(Dir::Right, PaneId(2), AREA, MIN));
        assert!(w.toggle_sidebar());
        assert!(!w.sidebar_expanded);
        assert!(w.toggle_sidebar());
        assert!(w.sidebar_expanded);
    }

    #[test]
    fn a_narrow_window_collapses_the_sidebar_for_a_while() {
        let mut w = Window::default();
        w.tabs.push(Tab::new("t".into(), PaneId(1)));
        assert!(w.tabs[0].split(Dir::Right, PaneId(2), AREA, MIN));
        assert!(!w.fit_width(1200.0));
        assert!(w.sidebar_expanded);
        assert!(w.fit_width(790.0), "changed");
        assert!(!w.sidebar_expanded && w.chosen_expanded());
        // Back over the line, but not by enough to flip it back.
        assert!(!w.fit_width(820.0));
        assert!(!w.sidebar_expanded);
        assert!(w.fit_width(880.0), "changed");
        assert!(w.sidebar_expanded && w.narrow.is_none());

        // Collapsed by hand, it stays collapsed when the window widens.
        w.fit_width(700.0);
        assert!(w.toggle_sidebar() && w.sidebar_expanded);
        assert!(w.toggle_sidebar() && !w.sidebar_expanded);
        assert!(!w.chosen_expanded());
        w.fit_width(1000.0);
        assert!(!w.sidebar_expanded);
        // Expanded by hand while narrow, it stays until the window is
        // made narrow again.
        w.fit_width(700.0);
        assert!(w.toggle_sidebar() && w.sidebar_expanded);
        w.fit_width(750.0);
        assert!(w.sidebar_expanded && w.chosen_expanded());
        w.fit_width(1000.0);
        w.fit_width(799.0);
        assert!(!w.sidebar_expanded);
    }

    #[test]
    fn tabs_move_and_panes_get_tabs_of_their_own() {
        let mut w = Window::default();
        for n in 1..=3 {
            w.tabs.push(Tab::new(format!("t{n}"), PaneId(n)));
        }
        let names = |w: &Window| w.tabs.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        assert!(w.move_tab(1));
        assert_eq!(
            (names(&w), w.active),
            (vec!["t2".into(), "t1".into(), "t3".into()], 1)
        );
        assert!(w.move_tab(5), "as far as the end");
        assert_eq!(
            (names(&w), w.active),
            (vec!["t2".into(), "t3".into(), "t1".into()], 2)
        );
        assert!(!w.move_tab(1), "already last");
        assert!(w.move_tab(-9));
        assert_eq!(w.active, 0);
        assert!(!w.move_tab(-1), "already first");

        // A pane leaves its tab for a new one at the end, which shows.
        let mut w = Window::default();
        w.tabs.push(four());
        w.tabs.push(Tab::new("t5".into(), PaneId(5)));
        let before = w.clone();
        assert!(
            !w.pane_to_new_tab(PaneId(5), "x".into()),
            "alone in its tab"
        );
        assert!(!w.pane_to_new_tab(PaneId(9), "x".into()));
        assert_eq!(w, before);
        assert!(w.pane_to_new_tab(PaneId(3), "x".into()));
        assert_eq!(w.tabs[0].panes(), ids(&[1, 2, 4]));
        assert_eq!((w.tabs.len(), w.active), (3, 2));
        assert_eq!(
            (w.tabs[2].name.as_str(), w.tabs[2].panes()),
            ("x", ids(&[3]))
        );
    }
}
