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

impl Ev {
    /// The event for an OSC 777 notification title such as
    /// `blitz:<token>:done`, as written by `blitz-hook` with the pane's
    /// token. Other titles, and any without that token, are not attention
    /// events: program output can print them too.
    ///
    /// Newer hooks add the Claude Code session id, `blitz:<token>:done:<id>`,
    /// which comes back too. A title whose id is not a valid one is dropped.
    pub fn from_notify<'a>(title: &'a str, token: &str) -> Option<(Ev, Option<&'a str>)> {
        let rest = title
            .strip_prefix("blitz:")?
            .strip_prefix(token)?
            .strip_prefix(':')?;
        if token.is_empty() {
            return None;
        }
        let (state, session) = match rest.split_once(':') {
            Some((state, id)) if crate::hook::is_session_id(id) => (state, Some(id)),
            Some(_) => return None,
            None => (rest, None),
        };
        let ev = match state {
            "needs-you" => Ev::NeedsYou,
            "working" => Ev::Working,
            "done" => Ev::Done,
            "error" => Ev::Error { sticky: false },
            "idle" => Ev::Idle,
            _ => return None,
        };
        Some((ev, session))
    }

    /// The event for the session's root process exiting with `code`.
    pub fn from_exit(code: u32) -> Ev {
        // STATUS_CONTROL_C_EXIT: the user stopped it, which is not a failure.
        const CTRL_C_EXIT: u32 = 0xC000_013A;
        match code {
            0 | CTRL_C_EXIT => Ev::Idle,
            _ => Ev::Error { sticky: true },
        }
    }
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

/// The session to jump to: the one waiting longest among those that need
/// the user, then those with unseen results, then those that failed.
pub fn jump_target<T>(sessions: impl IntoIterator<Item = (T, Attn, Instant)>) -> Option<T> {
    sessions
        .into_iter()
        .filter_map(|(id, state, since)| {
            let rank = match state {
                Attn::NeedsYou => 0,
                Attn::DoneUnseen => 1,
                Attn::Error => 2,
                Attn::Working | Attn::Idle => return None,
            };
            Some((rank, since, id))
        })
        .min_by_key(|&(rank, since, _)| (rank, since))
        .map(|(_, _, id)| id)
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

    const TOKEN: &str = "0f1e2d3c";

    #[test]
    fn notify_titles() {
        let ev = |s: &str| Ev::from_notify(s, TOKEN).map(|(ev, _)| ev);
        assert_eq!(ev("blitz:0f1e2d3c:needs-you"), Some(Ev::NeedsYou));
        assert_eq!(ev("blitz:0f1e2d3c:working"), Some(Ev::Working));
        assert_eq!(ev("blitz:0f1e2d3c:done"), Some(Ev::Done));
        assert_eq!(
            ev("blitz:0f1e2d3c:error"),
            Some(Ev::Error { sticky: false })
        );
        assert_eq!(ev("blitz:0f1e2d3c:idle"), Some(Ev::Idle));
        assert_eq!(ev("blitz:0f1e2d3c:bogus"), None);
        assert_eq!(ev("Build finished"), None);
        assert_eq!(ev(""), None);
    }

    const SESSION: &str = "0b8f6a3e-1c2d-4e5f-9a7b-3c4d5e6f7a8b";

    #[test]
    fn notify_titles_with_a_session() {
        fn ev(s: &str) -> Option<(Ev, Option<&str>)> {
            Ev::from_notify(s, TOKEN)
        }
        assert_eq!(ev("blitz:0f1e2d3c:done"), Some((Ev::Done, None)));
        assert_eq!(
            ev(&format!("blitz:0f1e2d3c:done:{SESSION}")),
            Some((Ev::Done, Some(SESSION)))
        );
        assert_eq!(
            ev(&format!("blitz:0f1e2d3c:idle:{SESSION}")),
            Some((Ev::Idle, Some(SESSION)))
        );
        for bad in [
            "blitz:0f1e2d3c:done:",
            "blitz:0f1e2d3c:done:abc",
            &format!("blitz:0f1e2d3c:done:{SESSION}:x"),
            &format!("blitz:0f1e2d3c:done:{}", SESSION.replacen('0', ";", 1)),
            &format!("blitz:0f1e2d3c:bogus:{SESSION}"),
        ] {
            assert_eq!(ev(bad), None, "{bad}");
        }
    }

    /// Output can print the hook's sequence, but not the pane's token.
    #[test]
    fn notify_titles_need_the_token() {
        for title in [
            "blitz:needs-you",
            "blitz:working",
            "blitz::done",
            "blitz:0f1e2d3:done",
            "blitz:0f1e2d3c0:done",
            "blitz:0F1E2D3C:done",
            "blitz:0f1e2d3cdone",
        ] {
            assert_eq!(Ev::from_notify(title, TOKEN), None, "{title}");
        }
        assert_eq!(Ev::from_notify("blitz::done", ""), None);
        assert_eq!(events("\x1b]777;notify;blitz:needs-you;Bash: x\x07"), []);
    }

    #[test]
    fn jump_prefers_needs_you_then_unseen_then_errors_oldest_first() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let mut s = vec![
            (1, Attn::Error, at(0)),
            (2, Attn::DoneUnseen, at(1)),
            (3, Attn::Working, at(0)),
            (4, Attn::NeedsYou, at(5)),
            (5, Attn::NeedsYou, at(3)),
            (6, Attn::Idle, at(0)),
        ];
        assert_eq!(jump_target(s.clone()), Some(5));
        s.retain(|x| x.1 != Attn::NeedsYou);
        assert_eq!(jump_target(s.clone()), Some(2));
        s.retain(|x| x.1 != Attn::DoneUnseen);
        assert_eq!(jump_target(s.clone()), Some(1));
        s.retain(|x| x.1 != Attn::Error);
        assert_eq!(jump_target(s), None);
    }

    /// The events a program's output turns into, the way the app reads
    /// them: notifications through the terminal, then the exit code.
    fn events(out: &str) -> Vec<Ev> {
        let mut t = vt::Terminal::new(vt::Options {
            cols: 40,
            rows: 5,
            ..Default::default()
        });
        t.feed(out.as_bytes());
        let mut evs = Vec::new();
        t.take_events(&mut evs);
        evs.into_iter()
            .filter_map(|e| match e {
                vt::Event::Notify { title, .. } => Ev::from_notify(&title, TOKEN).map(|(ev, _)| ev),
                _ => None,
            })
            .collect()
    }

    fn notify(state: &str) -> String {
        format!("\x1b]777;notify;blitz:{TOKEN}:{state};msg\x07")
    }

    /// Each attention rule, driven by hook notifications as a program
    /// prints them and by exit codes.
    #[test]
    fn rules_from_terminal_output() {
        let run = |start: Attn, steps: &[(&str, bool, Attn)]| {
            let mut p = pane(start);
            for &(input, attended, want) in steps {
                let evs = match input {
                    "seen" => vec![Ev::Attended],
                    "exit 0" => vec![Ev::from_exit(0)],
                    "exit 1" => vec![Ev::from_exit(1)],
                    s => events(&notify(s)),
                };
                assert_eq!(evs.len(), 1, "{input}");
                p.apply(evs[0], attended, Instant::now());
                assert_eq!(p.state, want, "{start:?} after {input}");
            }
        };
        use Attn::*;
        // Asked while away, then seen: back to working.
        run(
            Working,
            &[("needs-you", AWAY, NeedsYou), ("seen", HERE, Working)],
        );
        // Asked while watched: nothing to show.
        run(Idle, &[("needs-you", HERE, Idle)]);
        // Finished while away, then seen.
        run(Working, &[("done", AWAY, DoneUnseen), ("seen", HERE, Idle)]);
        // The user answered in the session.
        run(NeedsYou, &[("working", AWAY, Working)]);
        // A failed exit stays red whatever comes next.
        run(
            Working,
            &[
                ("exit 1", AWAY, Error),
                ("working", AWAY, Error),
                ("idle", HERE, Error),
                ("seen", HERE, Error),
            ],
        );
        // A clean exit is not a failure.
        run(Working, &[("exit 0", AWAY, Idle)]);
        // A needs-you over an unseen result gives the result back once
        // answered, and that is seen at once.
        run(
            DoneUnseen,
            &[("needs-you", AWAY, NeedsYou), ("seen", HERE, Idle)],
        );
        // A hook error while away, cleared when seen; idle is idle.
        run(
            Working,
            &[
                ("error", AWAY, Error),
                ("seen", HERE, Idle),
                ("working", AWAY, Working),
                ("idle", AWAY, Idle),
            ],
        );
    }

    #[test]
    fn other_notifications_are_not_attention() {
        assert!(events("\x1b]777;notify;Build;done\x07").is_empty());
        assert!(events("\x1b]9;build done\x07").is_empty());
        assert_eq!(events(&notify("done")), [Ev::Done]);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(Ev::from_exit(0), Ev::Idle);
        assert_eq!(Ev::from_exit(0xC000_013A), Ev::Idle);
        assert_eq!(Ev::from_exit(1), Ev::Error { sticky: true });
        assert_eq!(Ev::from_exit(0xC000_0005), Ev::Error { sticky: true });
    }
}
