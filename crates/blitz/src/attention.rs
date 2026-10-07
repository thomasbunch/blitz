//! Per-session attention state: whether a session needs the user.

use std::time::{Duration, Instant};

use vt::PromptMark;

use crate::render::chrome::elapsed;

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
    /// The user sent Claude Code a prompt, which starts a turn.
    Working,
    /// Claude Code's title shows it working: on with the turn it is in.
    /// A question stays, as with [`Ev::Quiet`]: only an answer or a hook
    /// ends one.
    Busy,
    Done,
    /// Claude Code's title stopped showing work. That ends a turn, but a
    /// question stays: while one waits the title looks the same.
    Quiet,
    Error {
        sticky: bool,
    },
    Idle,
    /// blitz's own prompt came back, so Claude Code is gone: whatever it
    /// was doing or asking is over. A bell and a result stay.
    Exited,
    /// The user is now looking at the pane.
    Attended,
    /// The user typed, pasted or clicked into the pane, which answers what
    /// it asked.
    Answered,
    /// Claude Code started and its hooks report. Changes nothing.
    Ready,
}

impl Ev {
    /// The event for an OSC 777 notification title such as
    /// `blitz:<token>:done`, as written by `blitz-hook` with the pane's
    /// token. Other titles, and any without that token, are not attention
    /// events: program output can print them too.
    ///
    /// Newer hooks add the Claude Code session id, `blitz:<token>:done:<id>`,
    /// which comes back too. A title whose id is not a valid one is dropped.
    /// The protocol stamp at the end, `:v2`, is read by [`notify_protocol`].
    pub fn from_notify<'a>(title: &'a str, token: &str) -> Option<(Ev, Option<&'a str>)> {
        let rest = notify_protocol(title)
            .0
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
            "ready" => Ev::Ready,
            _ => return None,
        };
        Some((ev, session))
    }

    /// The event for the session's root process exiting with `code`.
    pub fn from_exit(code: u32) -> Ev {
        match code {
            // The user stopped it, which is not a failure.
            0 | CTRL_C_EXIT => Ev::Idle,
            _ => Ev::Error { sticky: true },
        }
    }
}

/// What Claude Code's terminal title says: whether it is working (◐ or ◑,
/// which it turns while it works) or not (✳), and the title after that
/// mark, which names the task. `None` for any other title.
pub fn claude_title(title: &str) -> Option<(bool, &str)> {
    let mut chars = title.chars();
    let working = match chars.next()? {
        '\u{25D0}' | '\u{25D1}' => true,
        '\u{2733}' => false,
        _ => return None,
    };
    Some((working, chars.as_str().strip_prefix(' ')?.trim()))
}

/// A hook title without its protocol stamp, and the protocol: `N` from a
/// `:vN` ending, or 1 for titles from before the stamp.
pub fn notify_protocol(title: &str) -> (&str, u32) {
    match title.rsplit_once(":v") {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            (head, n.parse().unwrap_or(u32::MAX))
        }
        _ => (title, 1),
    }
}
/// STATUS_CONTROL_C_EXIT: a console program stopped with Ctrl+C.
pub const CTRL_C_EXIT: u32 = 0xC000_013A;

/// How long a shell command runs before its end is news.
pub const LONG_COMMAND: Duration = Duration::from_secs(10);

/// The command a shell with blitz's integration is running, as its prompt
/// marks tell: when it started and how it ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Command {
    /// blitz's own prompt shows, so the next start mark is the shell's.
    /// Other shells print marks too, but never end a command with one.
    prompt: bool,
    /// From the mark at its start until blitz's next prompt.
    pub running: Option<Instant>,
    /// The exit code its end mark gave.
    code: Option<i32>,
    /// A hook spoke while it ran: Claude Code says how it went itself.
    pub hooked: bool,
}

impl Command {
    /// Feeds a prompt mark. At blitz's own prompt, which program output
    /// cannot fake, gives the event and message for a command that ran at
    /// least [`LONG_COMMAND`] and reported its code: done when it worked or
    /// was stopped with Ctrl+C, else an error.
    pub fn mark(&mut self, m: PromptMark, now: Instant) -> Option<(Ev, String)> {
        match m {
            PromptMark::C if self.prompt => {
                *self = Command {
                    running: Some(now),
                    ..Command::default()
                };
            }
            PromptMark::D(code) => self.code = code,
            PromptMark::A { blitz: true } => {
                let ended = std::mem::replace(self, Command::at_prompt());
                let took = now.saturating_duration_since(ended.running?);
                if took < LONG_COMMAND || ended.hooked {
                    return None;
                }
                let code = ended.code? as u32;
                let ev = match code {
                    0 | 130 | CTRL_C_EXIT => Ev::Done,
                    _ => Ev::Error { sticky: false },
                };
                let msg = format!("{} \u{b7} {}", exit_text(code), elapsed(took));
                return Some((ev, msg));
            }
            _ => {}
        }
        None
    }

    /// At blitz's prompt, with nothing running.
    fn at_prompt() -> Command {
        Command {
            prompt: true,
            ..Command::default()
        }
    }
}

/// What closing a session would cut short, if anything: Claude Code
/// working or waiting for the user, or a command its shell is running.
/// Claude Code is not busy at its own prompt, where closing loses nothing
/// a resume does not bring back: `claude` when the session runs it, and a
/// hook that spoke says so too.
pub fn busy(state: Attn, cmd: &Command, claude: bool) -> Option<&'static str> {
    match state {
        Attn::Working => Some("working"),
        Attn::NeedsYou => Some("waiting for you"),
        _ if cmd.running.is_some() && !cmd.hooked && !claude => Some("running a command"),
        _ => None,
    }
}

/// An exit code as people read it: `exit 1` or `exit -1`, a Windows status
/// code in hex, and the common ways a program dies by name.
pub fn exit_text(code: u32) -> String {
    match code {
        CTRL_C_EXIT => "Ctrl+C".into(),
        0xC000_0005 => "access violation".into(),
        0xC000_00FD => "stack overflow".into(),
        // A fast fail, which is how abort() and a Rust panic set to abort end.
        0xC000_0409 => "aborted".into(),
        _ if (code as i32).unsigned_abs() <= 0xFFFF => format!("exit {}", code as i32),
        _ => format!("exit 0x{code:08X}"),
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
    /// When the turn under way began. A question in the middle of it
    /// leaves it open.
    pub turn: Option<Instant>,
    /// How long the last turn took.
    pub took: Option<Duration>,
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
            turn: None,
            took: None,
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
        let mut step = ev;
        while self.step(step, attended) && attended {
            step = Ev::Attended;
        }
        match ev {
            Ev::Working => self.turn = Some(now),
            Ev::Busy => {
                self.turn.get_or_insert(now);
            }
            Ev::Done | Ev::Idle | Ev::Error { .. } | Ev::Exited => self.end_turn(now),
            Ev::Quiet if before == Attn::Working => self.end_turn(now),
            _ => {}
        }
        if self.state == before {
            return false;
        }
        self.since = now;
        true
    }

    fn end_turn(&mut self, now: Instant) {
        if let Some(t) = self.turn.take() {
            self.took = Some(now.saturating_duration_since(t));
        }
    }

    fn step(&mut self, ev: Ev, attended: bool) -> bool {
        use Attn::*;
        if self.sticky {
            // Looking at a dead session is all that is left to do.
            self.seen |= matches!(ev, Ev::Attended | Ev::Answered);
            return false;
        }
        let next = match ev {
            Ev::Ready => return false,
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
            // The title cannot answer a question: any program can print
            // one. The answer finds Claude Code at work.
            Ev::Busy if self.state == NeedsYou => {
                self.prev = Working;
                return false;
            }
            Ev::Working | Ev::Busy => Working,
            Ev::Done if attended => Idle,
            Ev::Done => DoneUnseen,
            Ev::Quiet => match self.state {
                Working if attended => Idle,
                Working => DoneUnseen,
                // Answered, it finds the turn over.
                NeedsYou => {
                    if self.prev == Working {
                        self.prev = DoneUnseen;
                    }
                    return false;
                }
                s => s,
            },
            Ev::Error { sticky } => {
                self.sticky = sticky;
                if attended && !sticky { Idle } else { Error }
            }
            Ev::Idle => Idle,
            Ev::Exited => match self.state {
                Working => Idle,
                NeedsYou if !self.bell => Idle,
                // A bell stays, but looking at it must not bring back a
                // Claude Code at work that has gone.
                NeedsYou => {
                    if self.prev == Working {
                        self.prev = Idle;
                    }
                    return false;
                }
                s => s,
            },
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
    fn claude_titles() {
        assert_eq!(
            claude_title("\u{2733} Claude Code"),
            Some((false, "Claude Code"))
        );
        assert_eq!(
            claude_title("\u{25D0} Fix the login"),
            Some((true, "Fix the login"))
        );
        assert_eq!(
            claude_title("\u{25D1} Fix the login"),
            Some((true, "Fix the login"))
        );
        for other in [
            "",
            "\u{2733}",
            "\u{2733}Claude",
            "pwsh",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            "Claude \u{2733} Code",
        ] {
            assert_eq!(claude_title(other), None, "{other:?}");
        }
    }

    /// The title says when a turn ends, which no hook does when the user
    /// interrupts it.
    #[test]
    fn title_marks_start_and_end_work() {
        let mut p = pane(Attn::Idle);
        assert!(p.apply(Ev::Busy, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::Working);
        assert!(p.apply(Ev::Quiet, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::DoneUnseen);
        // A quiet title says nothing about a pane that was not working.
        for state in [Attn::Idle, Attn::DoneUnseen, Attn::Error] {
            let mut p = pane(state);
            assert!(!p.apply(Ev::Quiet, AWAY, Instant::now()));
            assert_eq!(p.state, state);
        }
        let mut p = pane(Attn::Working);
        assert!(p.apply(Ev::Quiet, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    /// A title, which any program can print, never ends a question: the
    /// answer then finds Claude Code at work, or, after the title went
    /// quiet, the turn over.
    #[test]
    fn title_marks_around_a_question() {
        let mut p = pane(Attn::Working);
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        p.apply(Ev::Quiet, AWAY, Instant::now());
        assert!(!p.apply(Ev::Busy, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::NeedsYou);
        assert!(p.apply(Ev::Answered, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Working);

        let mut p = pane(Attn::Working);
        p.apply(Ev::NeedsYou, AWAY, Instant::now());
        assert!(!p.apply(Ev::Quiet, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::NeedsYou);
        assert!(p.apply(Ev::Answered, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
        // Once answered, the title shows it at work again.
        assert!(p.apply(Ev::Busy, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Working);
    }

    /// Claude Code is gone once the shell's prompt is back, however it
    /// ended: whatever it was doing or asking is over.
    #[test]
    fn the_prompt_coming_back_ends_claude() {
        for state in [Attn::Working, Attn::NeedsYou] {
            let mut p = pane(Attn::Working);
            if state == Attn::NeedsYou {
                p.apply(Ev::NeedsYou, AWAY, Instant::now());
            }
            assert!(p.apply(Ev::Exited, AWAY, Instant::now()));
            assert_eq!(p.state, Attn::Idle);
        }
        // What the user has not seen yet stays.
        for state in [Attn::Idle, Attn::DoneUnseen, Attn::Error] {
            let mut p = pane(state);
            assert!(!p.apply(Ev::Exited, AWAY, Instant::now()));
            assert_eq!(p.state, state);
        }
        // So does a bell, which a command can ring just before its prompt.
        let mut p = pane(Attn::Idle);
        p.apply(Ev::Bell, AWAY, Instant::now());
        assert!(!p.apply(Ev::Exited, AWAY, Instant::now()));
        assert_eq!(p.state, Attn::NeedsYou);
        // Looking at one rung while Claude Code worked finds it gone.
        let mut p = pane(Attn::Working);
        p.apply(Ev::Bell, AWAY, Instant::now());
        assert!(!p.apply(Ev::Exited, AWAY, Instant::now()));
        assert!(p.apply(Ev::Attended, HERE, Instant::now()));
        assert_eq!(p.state, Attn::Idle);
    }

    /// A turn runs from the prompt to the result, questions and all.
    #[test]
    fn a_turn_spans_its_questions() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let mut p = PaneAttn::new(t0);
        p.apply(Ev::Working, AWAY, at(0));
        p.apply(Ev::Busy, AWAY, at(1));
        p.apply(Ev::NeedsYou, AWAY, at(10));
        // The title stops while the question waits.
        p.apply(Ev::Quiet, AWAY, at(10));
        p.apply(Ev::Answered, HERE, at(20));
        p.apply(Ev::Busy, HERE, at(21));
        assert_eq!((p.state, p.turn), (Attn::Working, Some(at(0))));
        p.apply(Ev::Done, AWAY, at(90));
        assert_eq!((p.turn, p.took), (None, Some(Duration::from_secs(90))));
        // The hook's done after the title's adds nothing.
        p.apply(Ev::Done, AWAY, at(95));
        assert_eq!(p.took, Some(Duration::from_secs(90)));

        // A new prompt starts a new turn, even with one left open by a
        // question the user turned down with Esc.
        p.apply(Ev::Working, AWAY, at(100));
        p.apply(Ev::NeedsYou, AWAY, at(110));
        p.apply(Ev::Answered, HERE, at(120));
        p.apply(Ev::Quiet, HERE, at(120));
        p.apply(Ev::Working, HERE, at(200));
        assert_eq!(p.turn, Some(at(200)));

        // With only the title to go by, each spell of work is a turn.
        let mut p = PaneAttn::new(t0);
        p.apply(Ev::Busy, AWAY, at(0));
        p.apply(Ev::Quiet, AWAY, at(30));
        assert_eq!(
            (p.state, p.took),
            (Attn::DoneUnseen, Some(Duration::from_secs(30)))
        );
        p.apply(Ev::Busy, AWAY, at(40));
        assert_eq!(p.turn, Some(at(40)));
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
                Ev::Busy,
                Ev::Quiet,
                Ev::Exited,
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
        assert_eq!(ev("ready"), Some(Ev::Ready));
        assert_eq!(ev("done:v2"), Some(Ev::Done));
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

    #[test]
    fn notify_protocol_stamps() {
        let t = format!("blitz:{TOKEN}:done:{SESSION}");
        assert_eq!(notify_protocol(&t), (t.as_str(), 1));
        assert_eq!(notify_protocol(&format!("{t}:v2")), (t.as_str(), 2));
        assert_eq!(notify_protocol(&format!("{t}:v10")), (t.as_str(), 10));
        // Not a stamp: kept, so the title is still checked whole.
        for odd in [format!("{t}:v"), format!("{t}:vx"), format!("{t}:v2a")] {
            assert_eq!(notify_protocol(&odd), (odd.as_str(), 1), "{odd}");
            assert_eq!(Ev::from_notify(&odd, TOKEN), None, "{odd}");
        }
        assert_eq!(
            Ev::from_notify(&format!("{t}:v2"), TOKEN),
            Some((Ev::Done, Some(SESSION)))
        );
        assert_eq!(Ev::from_notify(&format!("{t}:v2:v2"), TOKEN), None);
    }

    /// Ready says the hooks report; whatever the session was doing, it
    /// still is.
    #[test]
    fn ready_changes_nothing() {
        for state in [Attn::Idle, Attn::Working, Attn::DoneUnseen, Attn::NeedsYou] {
            for attended in [AWAY, HERE] {
                let mut p = pane(state);
                assert!(!p.apply(Ev::Ready, attended, Instant::now()), "{state:?}");
                assert_eq!(p.state, state);
            }
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

    /// A command as PowerShell marks it: started at `t0`, ended `secs`
    /// later with `code`.
    fn command(secs: u64, code: Option<i32>) -> (Command, Option<(Ev, String)>) {
        let t0 = Instant::now();
        let mut c = Command::at_prompt();
        assert_eq!(c.mark(PromptMark::C, t0), None);
        assert_eq!(c.running, Some(t0));
        assert_eq!(c.mark(PromptMark::D(code), t0), None);
        let end = c.mark(
            PromptMark::A { blitz: true },
            t0 + Duration::from_secs(secs),
        );
        (c, end)
    }

    #[test]
    fn a_long_command_ends_done_or_error() {
        assert_eq!(
            command(134, Some(101)).1,
            Some((Ev::Error { sticky: false }, "exit 101 \u{b7} 2m".into()))
        );
        assert_eq!(
            command(10, Some(0)).1,
            Some((Ev::Done, "exit 0 \u{b7} 10s".into()))
        );
        // Stopped with Ctrl+C, as Windows and as a POSIX shell say it.
        assert_eq!(
            command(60, Some(CTRL_C_EXIT as i32)).1,
            Some((Ev::Done, "Ctrl+C \u{b7} 1m".into()))
        );
        assert_eq!(command(60, Some(130)).1.map(|e| e.0), Some(Ev::Done));
        // The next prompt ends it either way.
        assert_eq!(command(60, Some(1)).0, Command::at_prompt());
    }

    #[test]
    fn a_short_command_or_one_with_no_code_is_not_news() {
        assert_eq!(command(9, Some(1)), (Command::at_prompt(), None));
        assert_eq!(command(600, None), (Command::at_prompt(), None));
    }

    /// Claude Code's hooks say how its turns went; quitting it after an
    /// hour is not a finished command.
    #[test]
    fn a_command_a_hook_spoke_in_is_not_news() {
        let t0 = Instant::now();
        let mut c = Command::at_prompt();
        c.mark(PromptMark::C, t0);
        c.hooked = true;
        c.mark(PromptMark::D(Some(0)), t0);
        let end = c.mark(
            PromptMark::A { blitz: true },
            t0 + Duration::from_secs(3600),
        );
        assert_eq!(end, None);
        // The next command starts with a clean slate.
        c.mark(PromptMark::C, t0);
        assert!(!c.hooked);
    }

    /// Program output can print any mark but blitz's own prompt start, so
    /// only that ends a command, and a second start does not restart it.
    #[test]
    fn only_blitz_prompts_end_a_command() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(30);
        let mut c = Command::at_prompt();
        c.mark(PromptMark::C, t0);
        c.mark(PromptMark::D(Some(3)), t0);
        assert_eq!(c.mark(PromptMark::A { blitz: false }, later), None);
        assert_eq!(c.mark(PromptMark::C, later), None);
        assert_eq!(c.running, Some(t0));
        // The shell's own end mark comes last and is the one that counts.
        c.mark(PromptMark::D(Some(0)), later);
        let end = c.mark(PromptMark::A { blitz: true }, later);
        assert_eq!(end, Some((Ev::Done, "exit 0 \u{b7} 30s".into())));
        // A prompt with no command before it.
        assert_eq!(c.mark(PromptMark::A { blitz: true }, later), None);
    }

    /// A shell without blitz's prompt, as nushell, prints the marks too,
    /// but never ends a command with blitz's prompt, so none starts one.
    #[test]
    fn only_a_command_after_a_blitz_prompt_runs() {
        let t0 = Instant::now();
        let mut c = Command::default();
        for m in [PromptMark::A { blitz: false }, PromptMark::B, PromptMark::C] {
            assert_eq!(c.mark(m, t0), None);
        }
        assert_eq!(c.running, None);
        c.mark(PromptMark::A { blitz: true }, t0);
        c.mark(PromptMark::C, t0);
        assert_eq!(c.running, Some(t0));
    }

    #[test]
    fn busy_is_working_waiting_or_running_a_command() {
        let idle = Command::default();
        let running = Command {
            running: Some(Instant::now()),
            ..Command::default()
        };
        assert_eq!(busy(Attn::Working, &idle, true), Some("working"));
        assert_eq!(
            busy(Attn::NeedsYou, &running, true),
            Some("waiting for you")
        );
        assert_eq!(busy(Attn::Idle, &running, false), Some("running a command"));
        assert_eq!(
            busy(Attn::DoneUnseen, &running, false),
            Some("running a command")
        );
        for state in [Attn::Idle, Attn::DoneUnseen, Attn::Error] {
            assert_eq!(busy(state, &idle, false), None, "{state:?}");
        }
        // Claude Code at its prompt, resumed or after a turn.
        let hooked = Command {
            hooked: true,
            ..running
        };
        for (cmd, claude) in [(&running, true), (&hooked, false)] {
            assert_eq!(busy(Attn::Idle, cmd, claude), None);
            assert_eq!(busy(Attn::DoneUnseen, cmd, claude), None);
        }
    }

    #[test]
    fn exit_codes_read_as_people_say_them() {
        assert_eq!(exit_text(0), "exit 0");
        assert_eq!(exit_text(101), "exit 101");
        assert_eq!(exit_text(u32::MAX), "exit -1");
        assert_eq!(exit_text(0xFFFF), "exit 65535");
        assert_eq!(exit_text(0xC000_013A), "Ctrl+C");
        assert_eq!(exit_text(0xC000_0005), "access violation");
        assert_eq!(exit_text(0xC000_00FD), "stack overflow");
        assert_eq!(exit_text(0xC000_0409), "aborted");
        assert_eq!(exit_text(0xC000_0135), "exit 0xC0000135");
        assert_eq!(exit_text(0x8007_0005), "exit 0x80070005");
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
