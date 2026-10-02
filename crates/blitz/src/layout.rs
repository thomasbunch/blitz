//! Tabs, and the binary split tree inside each tab.

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
