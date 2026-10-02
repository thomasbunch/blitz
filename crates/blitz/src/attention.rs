//! Per-session attention state: whether a session needs the user.

use std::time::Instant;

/// Ordered by priority, lowest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Attn {
    #[default]
    Idle,
    Working,
    DoneUnseen,
    Error,
    NeedsYou,
}

/// Inputs to the state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    NeedsYou,
    Working,
    Done,
    Error {
        sticky: bool,
    },
    Idle,
    /// The user is now looking at the pane.
    Attended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneAttn {
    pub state: Attn,
    /// When `state` last changed.
    pub since: Instant,
}

impl PaneAttn {
    pub fn new(now: Instant) -> Self {
        Self {
            state: Attn::Idle,
            since: now,
        }
    }
}
