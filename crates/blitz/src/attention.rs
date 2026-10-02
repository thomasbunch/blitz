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
    /// What to go back to once a needs-you has been seen.
    prev: Attn,
    /// Set when the process died; nothing changes the state after that.
    sticky: bool,
}

impl PaneAttn {
    pub fn new(now: Instant) -> Self {
        Self {
            state: Attn::Idle,
            since: now,
            prev: Attn::Idle,
            sticky: false,
        }
    }

    /// Feeds one event. `attended` says whether the user is looking at the
    /// pane right now (window focused, tab active, pane focused). Returns
    /// true when the state changed, which is when side effects should fire.
    pub fn apply(&mut self, ev: Ev, attended: bool, now: Instant) -> bool {
        let before = self.state;
        let attended = attended || ev == Ev::Attended;
        // A change the user is watching is seen at once, and seeing a
        // restored state can clear it too (needs-you over done-unseen ends
        // at idle), so repeat until nothing moves. This settles within
        // three steps.
        let mut ev = ev;
        while self.step(ev, attended) && attended {
            ev = Ev::Attended;
        }
        if self.state == before {
            return false;
        }
        self.since = now;
        true
    }

    fn step(&mut self, ev: Ev, attended: bool) -> bool {
        use Attn::*;
        if self.sticky {
            return false;
        }
        let next = match ev {
            Ev::Attended => match self.state {
                NeedsYou => self.prev,
                DoneUnseen | Error => Idle,
                s => s,
            },
            // The user is already looking at it.
            Ev::NeedsYou if attended => return false,
            Ev::NeedsYou => {
                if self.state != NeedsYou {
                    self.prev = self.state;
                }
                NeedsYou
            }
            Ev::Working => Working,
            Ev::Done if attended => Idle,
            Ev::Done => DoneUnseen,
            Ev::Error { sticky } => {
                self.sticky = sticky;
                if attended && !sticky { Idle } else { Error }
            }
            Ev::Idle => Idle,
        };
        if next == self.state {
            return false;
        }
        self.state = next;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const AWAY: bool = false;
    const HERE: bool = true;

    fn pane(state: Attn) -> PaneAttn {
        let mut p = PaneAttn::new(Instant::now());
        p.state = state;
        p
    }

    #[test]
    fn needs_you_then_seen_restores_working() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::NeedsYou, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::NeedsYou);
        assert!(p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Working);
    }

    #[test]
    fn needs_you_while_watched_is_ignored() {
        let mut p = pane(Attn::Idle);
        assert!(!p.apply(Ev::NeedsYou, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    #[test]
    fn done_unseen_until_attended() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::Done, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::DoneUnseen);
        assert!(p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    #[test]
    fn done_while_watched_is_idle() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::Done, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    #[test]
    fn prompt_submit_answers_needs_you() {
        let mut p = pane(Attn::Working);
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        assert!(p.apply(Ev::Working, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::Working);
    }

    #[test]
    fn sticky_error_ignores_everything() {
        for start in [Attn::Idle, Attn::Working, Attn::NeedsYou] {
            let mut p = pane(start);
            assert!(p.apply(Ev::Error { sticky: true }, HERE, Instant::now()));
            assert_eq!(p.state, Attn::Error);
            for ev in [
                Ev::NeedsYou,
                Ev::Working,
                Ev::Done,
                Ev::Error { sticky: false },
                Ev::Idle,
                Ev::Attended,
            ] {
                assert!(!p.apply(ev, AWAY, Instant::now()), "{ev:?}");
                assert!(!p.apply(ev, HERE, Instant::now()), "{ev:?}");
                assert_eq!(p.state, Attn::Error);
            }
        }
    }

    #[test]
    fn error_clears_when_seen_unless_sticky() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::Error { sticky: false }, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::Error);
        assert!(p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);

        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::Error { sticky: false }, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    #[test]
    fn needs_you_over_done_unseen_ends_idle_when_seen() {
        let mut p = pane(Attn::DoneUnseen);
        assert!(p.apply(Ev::NeedsYou, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::NeedsYou);
        // Restoring done-unseen while watched is itself seen.
        assert!(p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    #[test]
    fn repeated_events_keep_since() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(5);
        let mut p = PaneAttn::new(t0);
        assert!(p.apply(Ev::Working, AWAY, t0));
        assert!(!p.apply(Ev::Working, AWAY, t1));
        assert_eq!(p.since, t0);
        assert!(p.apply(Ev::NeedsYou, AWAY, t1));
        assert!(!p.apply(Ev::NeedsYou, AWAY, t0));
        assert_eq!(p.since, t1);
    }

    #[test]
    fn priority_order() {
        use Attn::*;
        assert!(Idle < Working && Working < DoneUnseen);
        assert!(DoneUnseen < Error && Error < NeedsYou);
    }
}
