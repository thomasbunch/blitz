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
    /// Claude Code asks the user something. Only an answer ends it.
    NeedsYou,
    /// A bell: it needs the user until they look at the pane.
    Bell,
    Working,
    Done,
    Error {
        sticky: bool,
    },
    Idle,
    /// The user is now looking at the pane.
    Attended,
    /// The user typed, pasted or clicked into the pane, which answers what
    /// it asked.
    Answered,
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
    /// The user has looked at `state` since it changed. A needs-you can
    /// stay seen but unanswered, and so can an error that sticks.
    pub seen: bool,
    /// What to go back to once a needs-you is answered.
    prev: Attn,
    /// The needs-you came from a bell, which looking at the pane answers.
    bell: bool,
    /// Set when the process died; nothing changes the state after that.
    sticky: bool,
}

impl PaneAttn {
    pub fn new(now: Instant) -> Self {
        Self {
            state: Attn::Idle,
            since: now,
            seen: false,
            prev: Attn::Idle,
            bell: false,
            sticky: false,
        }
    }

    /// Feeds one event. `attended` says whether the user is looking at the
    /// pane right now (window focused, tab active, pane focused). Returns
    /// true when the state changed, which is when side effects should fire.
    pub fn apply(&mut self, ev: Ev, attended: bool, now: Instant) -> bool {
        let before = self.state;
        let attended = attended || matches!(ev, Ev::Attended | Ev::Answered);
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
            // Looking at a dead session is all that is left to do.
            self.seen |= matches!(ev, Ev::Attended | Ev::Answered);
            return false;
        }
        let next = match ev {
            Ev::Attended | Ev::Answered => match self.state {
                NeedsYou if ev == Ev::Answered || self.bell => self.prev,
                // Seen, but still waiting for an answer.
                NeedsYou => {
                    self.seen = true;
                    return false;
                }
                DoneUnseen | Error => Idle,
                s => s,
            },
            // The user is already looking at it, or a question waits
            // already, which a bell must not turn into one of its own.
            Ev::Bell if attended || self.state == NeedsYou => return false,
            Ev::NeedsYou | Ev::Bell => {
                if self.state != NeedsYou {
                    self.prev = self.state;
                }
                self.bell = ev == Ev::Bell;
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
        self.seen = false;
        true
    }
}

/// The session to jump to: the one waiting longest among those that need
/// the user and were not seen yet, then those seen but not answered, then
/// those with unseen results, then those that failed. An error that stuck
/// and was seen is left out: it stays red until its pane is closed.
pub fn jump_target<T>(sessions: impl IntoIterator<Item = (T, PaneAttn)>) -> Option<T> {
    sessions
        .into_iter()
        .filter_map(|(id, a)| {
            let rank = match (a.state, a.seen) {
                (Attn::NeedsYou, false) => 0,
                (Attn::NeedsYou, true) => 1,
                (Attn::DoneUnseen, _) => 2,
                (Attn::Error, false) => 3,
                _ => return None,
            };
            Some((rank, a.since, id))
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

    /// Looking at a question is not answering it.
    #[test]
    fn needs_you_is_seen_then_answered() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::NeedsYou, AWAY, Instant::now()));
        assert_eq!((p.state, p.seen), (Attn::NeedsYou, false));
        assert!(!p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!((p.state, p.seen), (Attn::NeedsYou, true));
        // Looking away and back changes nothing.
        assert!(!p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!((p.state, p.seen), (Attn::NeedsYou, true));
        assert!(p.apply(Ev::Answered, HERE, Instant::now()));
        assert_eq!((p.state, p.seen), (Attn::Working, false));
    }

    /// A question that comes while the user watches still waits for an
    /// answer, but it is seen.
    #[test]
    fn needs_you_while_watched_is_seen() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::NeedsYou, HERE, Instant::now()));
        assert_eq!((p.state, p.seen), (Attn::NeedsYou, true));
        assert!(p.apply(Ev::Answered, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Working);
    }

    /// A bell only needs the user to look.
    #[test]
    fn a_bell_is_answered_by_looking() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::Bell, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::NeedsYou);
        assert!(p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Working);
        // Nor does one the user hears while looking need anything.
        assert!(!p.apply(Ev::Bell, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Working);
    }

    /// A bell over a question does not make looking answer it.
    #[test]
    fn a_bell_leaves_a_question_waiting() {
        let mut p = pane(Attn::Working);
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        assert!(!p.apply(Ev::Bell, AWAY, Instant::now()));
        p.apply(Ev::Attended, HERE, Instant::now());
        assert_eq!((p.state, p.seen), (Attn::NeedsYou, true));
        // A question after a bell is a question.
        let mut p = pane(Attn::Idle);
        p.apply(Ev::Bell, AWAY, Instant::now());
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        p.apply(Ev::Attended, HERE, Instant::now());
        assert_eq!(p.state, Attn::NeedsYou);
    }

    /// Typing into a session with a result or a failure shown has seen it.
    #[test]
    fn answering_sees_a_result() {
        for state in [Attn::DoneUnseen, Attn::Error] {
            let mut p = pane(state);
            assert!(p.apply(Ev::Answered, HERE, Instant::now()));
            assert_eq!(p.state, Attn::Idle);
        }
        let mut p = pane(Attn::Working);
        assert!(!p.apply(Ev::Answered, HERE, Instant::now()));
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
                Ev::Bell,
                Ev::Answered,
            ] {
                assert!(!p.apply(ev, AWAY, Instant::now()), "{ev:?}");
                assert!(!p.apply(ev, HERE, Instant::now()), "{ev:?}");
                assert_eq!(p.state, Attn::Error);
            }
            // Seen, so a jump skips it from now on.
            assert!(p.seen);
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
    fn needs_you_over_an_error_ends_idle_when_seen() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::Error { sticky: false }, AWAY, Instant::now()));
        assert!(p.apply(Ev::NeedsYou, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::NeedsYou);
        // Back to the error, which is seen at once.
        assert!(p.apply(Ev::Answered, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    /// A hook error, then the process dying: the state stays an error,
    /// so nothing new fires, but now nothing clears it.
    #[test]
    fn a_crash_after_a_hook_error_makes_it_stick() {
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::Error { sticky: false }, AWAY, Instant::now()));
        assert!(!p.apply(Ev::Error { sticky: true }, AWAY, Instant::now()));
        assert!(!p.apply(Ev::Attended, HERE, Instant::now()));
        assert!(!p.apply(Ev::Working, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::Error);
    }

    #[test]
    fn session_end_clears_needs_you() {
        let mut p = pane(Attn::Working);
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        assert!(p.apply(Ev::Idle, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
        // Nothing to go back to once seen.
        assert!(!p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    /// A second question after the first was answered goes back to what
    /// came in between, not to what was there before the first.
    #[test]
    fn each_needs_you_remembers_what_it_interrupted() {
        let mut p = pane(Attn::DoneUnseen);
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        p.apply(Ev::Working, AWAY, Instant::now());
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        // Asked again while still waiting: the first interruption stands.
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        assert!(p.apply(Ev::Answered, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Working);
    }

    #[test]
    fn needs_you_over_done_unseen_ends_idle_when_answered() {
        let mut p = pane(Attn::DoneUnseen);
        assert!(p.apply(Ev::NeedsYou, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::NeedsYou);
        // Restoring done-unseen while watched is itself seen.
        assert!(p.apply(Ev::Answered, HERE, Instant::now()));
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

    /// As long as a real pane token: 128 bits in hex.
    const TOKEN: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f0";

    #[test]
    fn notify_titles() {
        let ev = |s: &str| Ev::from_notify(&format!("blitz:{TOKEN}:{s}"), TOKEN).map(|(ev, _)| ev);
        assert_eq!(ev("needs-you"), Some(Ev::NeedsYou));
        assert_eq!(ev("working"), Some(Ev::Working));
        assert_eq!(ev("done"), Some(Ev::Done));
        assert_eq!(ev("error"), Some(Ev::Error { sticky: false }));
        assert_eq!(ev("idle"), Some(Ev::Idle));
        assert_eq!(ev("bogus"), None);
        assert_eq!(ev(""), None);
        assert_eq!(Ev::from_notify("Build finished", TOKEN), None);
        assert_eq!(Ev::from_notify("", TOKEN), None);
    }

    const SESSION: &str = "0b8f6a3e-1c2d-4e5f-9a7b-3c4d5e6f7a8b";

    #[test]
    fn notify_titles_with_a_session() {
        let ev = |s: &str| {
            let title = format!("blitz:{TOKEN}:{s}");
            Ev::from_notify(&title, TOKEN).map(|(ev, id)| (ev, id.map(str::to_owned)))
        };
        assert_eq!(ev("done"), Some((Ev::Done, None)));
        assert_eq!(
            ev(&format!("done:{SESSION}")),
            Some((Ev::Done, Some(SESSION.to_owned())))
        );
        assert_eq!(
            ev(&format!("idle:{SESSION}")),
            Some((Ev::Idle, Some(SESSION.to_owned())))
        );
        for bad in [
            "done:",
            "done:abc",
            &format!("done:{SESSION}:x"),
            &format!("done:{}", SESSION.replacen('0', ";", 1)),
            &format!("done:{}", &SESSION[..20]),
            &format!("bogus:{SESSION}"),
        ] {
            assert_eq!(ev(bad), None, "{bad}");
        }
    }

    /// Output can print the hook's sequence, but not the pane's token.
    #[test]
    fn notify_titles_need_the_token() {
        for title in [
            "blitz:needs-you".to_owned(),
            "blitz:working".to_owned(),
            "blitz::done".to_owned(),
            format!("blitz:{}:done", &TOKEN[1..]),
            format!("blitz:{}:done", &TOKEN[..TOKEN.len() - 1]),
            format!("blitz:{TOKEN}0:done"),
            format!("blitz:{}:done", TOKEN.to_uppercase()),
            format!("blitz:{TOKEN}done"),
        ] {
            assert_eq!(Ev::from_notify(&title, TOKEN), None, "{title}");
        }
        assert_eq!(Ev::from_notify("blitz::done", ""), None);
        assert_eq!(events("\x1b]777;notify;blitz:needs-you;Bash: x\x07"), []);
    }

    /// A pane in `state` since `at`, seen or not.
    fn at(state: Attn, at: Instant, seen: bool) -> PaneAttn {
        PaneAttn {
            since: at,
            seen,
            ..pane(state)
        }
    }

    #[test]
    fn jump_prefers_needs_you_then_unseen_then_errors_oldest_first() {
        let t0 = Instant::now();
        let t = |s| t0 + Duration::from_secs(s);
        let mut s = vec![
            (1, at(Attn::Error, t(0), false)),
            (2, at(Attn::DoneUnseen, t(1), false)),
            (3, at(Attn::Working, t(0), false)),
            (4, at(Attn::NeedsYou, t(5), false)),
            (5, at(Attn::NeedsYou, t(3), false)),
            (6, at(Attn::Idle, t(0), false)),
            (7, at(Attn::NeedsYou, t(0), true)),
            (8, at(Attn::Error, t(0), true)),
        ];
        assert_eq!(jump_target(s.clone()), Some(5));
        // A question already seen comes after the ones not seen yet.
        s.retain(|x| !(x.1.state == Attn::NeedsYou && !x.1.seen));
        assert_eq!(jump_target(s.clone()), Some(7));
        s.retain(|x| x.1.state != Attn::NeedsYou);
        assert_eq!(jump_target(s.clone()), Some(2));
        s.retain(|x| x.1.state != Attn::DoneUnseen);
        assert_eq!(jump_target(s.clone()), Some(1));
        // An exited session the user has seen is not news.
        s.retain(|x| x.0 != 1);
        assert_eq!(jump_target(s), None);
    }

    #[test]
    fn jump_ties_go_to_the_first_listed() {
        let t0 = Instant::now();
        let s = [
            (7, at(Attn::NeedsYou, t0, false)),
            (8, at(Attn::NeedsYou, t0, false)),
        ];
        assert_eq!(jump_target(s), Some(7));
        assert_eq!(jump_target(Vec::<(u8, PaneAttn)>::new()), None);
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

    /// The sequence blitz-hook prints for `state`, session id and all.
    fn notify(state: &str) -> String {
        format!("\x1b]777;notify;blitz:{TOKEN}:{state}:{SESSION};msg\x07")
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
                    "answer" => vec![Ev::Answered],
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
        // Asked while away, seen, then answered: back to working.
        run(
            Working,
            &[
                ("needs-you", AWAY, NeedsYou),
                ("seen", HERE, NeedsYou),
                ("answer", HERE, Working),
            ],
        );
        // Asked while watched: it still waits for an answer.
        run(
            Idle,
            &[("needs-you", HERE, NeedsYou), ("answer", HERE, Idle)],
        );
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
            &[("needs-you", AWAY, NeedsYou), ("answer", HERE, Idle)],
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
        assert!(events("\x07\x1b]0;done\x07").is_empty(), "a bell or title");
        assert_eq!(events(&notify("done")), [Ev::Done]);
    }

    #[test]
    fn exit_codes() {
        assert_eq!(Ev::from_exit(0), Ev::Idle);
        assert_eq!(Ev::from_exit(0xC000_013A), Ev::Idle);
        assert_eq!(Ev::from_exit(1), Ev::Error { sticky: true });
        assert_eq!(Ev::from_exit(0xC000_0005), Ev::Error { sticky: true });
        // STILL_ACTIVE is a real exit code a program can return.
        assert_eq!(Ev::from_exit(259), Ev::Error { sticky: true });
        assert_eq!(Ev::from_exit(u32::MAX), Ev::Error { sticky: true });
    }
}
