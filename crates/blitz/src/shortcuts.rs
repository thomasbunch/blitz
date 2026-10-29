//! The keyboard shortcuts panel: every action with the keys that run it,
//! which match what was typed, and the new keys pressed for one.

use crate::config;
use crate::keymap::{self, ACTIONS, Action, Binding, Chord, Rebind};
use crate::render::chrome::SettingRow;

/// The headings the actions are listed under, in order.
const GROUPS: &[&str] = &[
    "Tabs",
    "Panes",
    "Selection",
    "Scrollback",
    "View",
    "Sessions",
    "blitz",
];

/// Why a key pressed for an action makes no chord.
const ALTGR: &str = "AltGr types a character with this key here";

/// Which of [`GROUPS`] an action is listed under.
fn group(a: Action) -> usize {
    match a {
        Action::NewTab
        | Action::CloseTab
        | Action::RenameTab
        | Action::CycleTab(_)
        | Action::MoveTab(_)
        | Action::GoToTab(_)
        | Action::LastTab => 0,
        Action::ClosePane
        | Action::ReopenClosed
        | Action::PaneToNewTab
        | Action::SplitRight
        | Action::SplitDown
        | Action::Focus(_)
        | Action::Resize(_)
        | Action::Swap(_)
        | Action::Zoom
        | Action::Equalize => 1,
        Action::Copy
        | Action::CopyUnindented
        | Action::Paste
        | Action::SelectAll
        | Action::SelectOutput
        | Action::QuickSelect
        | Action::SendText(_) => 2,
        Action::ScrollPage(_)
        | Action::ScrollEnd(_)
        | Action::JumpToPrompt(_)
        | Action::Find
        | Action::ClearScrollback
        | Action::Reset => 3,
        Action::FontSize(_)
        | Action::Fullscreen
        | Action::ThemePicker
        | Action::ToggleSidebar
        | Action::Files => 4,
        Action::NewClaude
        | Action::JumpToAttention
        | Action::GoToSession
        | Action::RenameSession
        | Action::ClaudeSetup => 5,
        Action::Palette
        | Action::Keys
        | Action::Settings
        | Action::OpenConfig
        | Action::OpenThemes
        | Action::Update
        | Action::SystemMenu
        | Action::ReportIssue => 6,
    }
}

/// The open keyboard shortcuts.
#[derive(Debug, Default)]
pub struct Panel {
    /// Typed text; actions that contain each typed word match.
    pub filter: String,
    /// The highlighted action, among the matching ones.
    pub sel: usize,
    /// The first list line shown in the last frame, so the list scrolls
    /// only when the highlight leaves it.
    pub top: usize,
    /// The action waiting for new keys, while one does.
    pub capture: Option<Capture>,
    /// The key that stopped the last wait, until another is pressed.
    pub stopped_by: Option<u16>,
    /// Why the last change was not saved.
    pub error: Option<String>,
}

/// An action waiting for new keys.
#[derive(Debug)]
pub struct Capture {
    pub action: Action,
    /// The chord pressed so far, or why the key pressed cannot make one.
    pub pressed: Option<Result<Chord, &'static str>>,
}

impl Panel {
    /// The actions that match the filter, by group, in palette order in
    /// each. The filter's words are looked for in each action's group,
    /// label, name and keys, with the `user` bindings.
    pub fn matches(&self, user: &[Binding]) -> Vec<Action> {
        let words: Vec<String> = (self.filter.split_whitespace())
            .map(str::to_lowercase)
            .collect();
        let mut found: Vec<Action> = (ACTIONS.iter())
            // Only Windows has a window menu to open.
            .filter(|a| cfg!(windows) || a.0 != Action::SystemMenu)
            .filter(|&&(a, name, label)| {
                let keys = keys(&keymap::chords(a, user));
                let text = format!("{} {label} {name} {keys}", GROUPS[group(a)]).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .map(|a| a.0)
            .collect();
        found.sort_by_key(|&a| group(a));
        found
    }

    pub fn selected(&self, user: &[Binding]) -> Option<Action> {
        self.matches(user).get(self.sel).copied()
    }

    /// Moves the highlight `by` rows, stopping at either end.
    pub fn move_by(&mut self, by: isize, user: &[Binding]) {
        let last = self.matches(user).len().saturating_sub(1);
        self.sel = self.sel.saturating_add_signed(by).min(last);
    }

    /// Keeps the highlight, and the wait for new keys, on their action when
    /// the keys change from `old` to `new`, which can take actions out of
    /// the list or put them in. When it goes, the highlight stays in the
    /// list and the wait stops.
    pub fn rekey(&mut self, old: &[Binding], new: &[Binding]) {
        let held = (self.capture.as_ref().map(|c| c.action)).or_else(|| self.selected(old));
        let found = self.matches(new);
        match found.iter().position(|&a| Some(a) == held) {
            Some(i) => self.sel = i,
            None => {
                self.sel = self.sel.min(found.len().saturating_sub(1));
                self.capture = None;
            }
        }
    }

    /// Waits for new keys for the highlighted action.
    pub fn wait(&mut self, user: &[Binding]) {
        self.capture = (self.selected(user)).map(|action| Capture {
            action,
            pressed: None,
        });
    }

    /// Whether a key pressed is the auto-repeat (`held`) of the key that
    /// stopped the last wait, which does nothing more: a held Backspace
    /// that gave keys back would go on to erase the search.
    pub fn repeats_stop(&mut self, vk: u16, held: bool) -> bool {
        if !held {
            self.stopped_by = None;
        }
        held && self.stopped_by == Some(vk)
    }

    /// The rows the chrome draws: each matching action with its keys.
    pub fn rows(&self, user: &[Binding]) -> Vec<SettingRow> {
        (self.matches(user).into_iter())
            .map(|a| {
                let (now, default) = (keymap::chords(a, user), keymap::chords(a, &[]));
                let capture = self.capture.as_ref().filter(|c| c.action == a);
                let shown = if now.is_empty() {
                    "No keys".into()
                } else {
                    keys(&now)
                };
                SettingRow {
                    group: GROUPS[group(a)],
                    label: keymap::label(a),
                    help: match capture {
                        Some(_) => format!(
                            "Press the keys to add to {}. Del gives its keys back to the \
                             program; Esc stops.",
                            keymap::label(a)
                        ),
                        None => format!("{shown}. In config.toml it is {}.", keymap::name(a)),
                    },
                    applies: config::NOW,
                    on: None,
                    value: match capture.map(|c| c.pressed) {
                        Some(Some(Ok(c))) => keymap::chord_label(c.0, c.1),
                        Some(_) => "press keys".into(),
                        None => keys(&now),
                    },
                    less: false,
                    more: false,
                    default: if default.is_empty() {
                        "none".into()
                    } else {
                        keys(&default)
                    },
                    note: capture.and_then(|c| match c.pressed? {
                        Ok(chord) => note(a, chord, user),
                        Err(why) => Some(why.into()),
                    }),
                    changed: now.len() != default.len() || now.iter().any(|c| !default.contains(c)),
                }
            })
            .collect()
    }

    /// A key pressed while an action waits for new keys, as the chord it
    /// makes, or `None` for one AltGr types with. Returns the changes to
    /// save: Enter adds the chord pressed to the action, unless blitz
    /// cannot take it; Del or Backspace gives that chord back to the
    /// program when it is the action's, or else every chord of the action;
    /// and Esc stops waiting. Any other key is the new chord pressed.
    pub fn press(&mut self, key: Option<Chord>, user: &[Binding]) -> Vec<Rebind> {
        let Some(cap) = &mut self.capture else {
            return Vec::new();
        };
        let a = cap.action;
        let own = match cap.pressed {
            Some(Ok(c)) if keymap::bound_to(c, user) == Some(a) => Some(c),
            _ => None,
        };
        let changes = match key {
            Some((_, 0x1b)) => Vec::new(),
            Some((0, 0x0d)) => match cap.pressed {
                Some(Ok(c)) if keymap::refused(c).is_none() && own.is_none() => {
                    vec![line(c, Some(a))]
                }
                _ if own.is_some() => Vec::new(),
                _ => return Vec::new(),
            },
            Some((0, 0x08 | 0x2e)) => match own {
                Some(c) => vec![line(c, None)],
                None => unbind(a, user),
            },
            _ => {
                cap.pressed = Some(key.ok_or(ALTGR));
                return Vec::new();
            }
        };
        self.capture = None;
        self.stopped_by = key.map(|(_, vk)| vk);
        changes
    }
}

/// What the meta line says of chord `c` pressed for action `a`: why blitz
/// cannot take it, or what it runs now.
fn note(a: Action, c: Chord, user: &[Binding]) -> Option<String> {
    if let Some(why) = keymap::refused(c) {
        return Some(why.into());
    }
    let keys = keymap::chord_label(c.0, c.1);
    match keymap::bound_to(c, user)? {
        b if b == a => Some(format!(
            "{keys} already runs {}; Del gives it back to the program",
            keymap::label(a)
        )),
        Action::SendText(_) => Some(format!(
            "{keys} types text now; Enter makes it {} instead",
            keymap::label(a)
        )),
        b => Some(format!(
            "{keys} runs {} now; Enter moves it here",
            keymap::label(b)
        )),
    }
}

/// The change that makes chord `c` run `want`, or with none go to the
/// program: no line where that is its default.
fn line(c: Chord, want: Option<Action>) -> Rebind {
    let name = want.map_or("none", keymap::name);
    let text = format!("{}={name}", keymap::chord_text(c));
    (c, (keymap::default_for(c) != want).then_some(text))
}

/// The changes that give every chord of `a` back to the program.
fn unbind(a: Action, user: &[Binding]) -> Vec<Rebind> {
    (keymap::chords(a, user).into_iter())
        .map(|c| line(c, None))
        .collect()
}

/// The changes that put the default keys of `a` back: the `user` lines
/// that bind other chords to it go, as do those that bind its own
/// default chords to something else.
pub fn reset(a: Action, user: &[Binding]) -> Vec<Rebind> {
    let mut chords: Vec<Chord> = (user.iter())
        .filter(|u| u.2 == Some(a))
        .map(|u| (u.0, u.1))
        .collect();
    for d in keymap::defaults(a) {
        if user.iter().any(|u| (u.0, u.1) == d) && !chords.contains(&d) {
            chords.push(d);
        }
    }
    chords.into_iter().map(|c| (c, None)).collect()
}

/// Chords as the panel lists them.
fn keys(chords: &[Chord]) -> String {
    (chords.iter())
        .map(|c| keymap::chord_label(c.0, c.1))
        .collect::<Vec<_>>()
        .join(" \u{b7} ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chord of a key as `config.toml` names it.
    fn chord(keys: &str) -> Chord {
        keymap::chord_of(&format!("{keys}=none")).expect(keys)
    }

    fn panel(filter: &str) -> Panel {
        Panel {
            filter: filter.into(),
            ..Panel::default()
        }
    }

    #[test]
    fn every_action_is_listed_once_in_its_group() {
        let list = panel("").matches(&[]);
        for &(a, ..) in ACTIONS {
            let n = list.iter().filter(|&&b| b == a).count();
            let menu = cfg!(windows) || a != Action::SystemMenu;
            assert_eq!(n, usize::from(menu), "{a:?}");
        }
        let groups: Vec<usize> = list.iter().map(|&a| group(a)).collect();
        assert!(groups.is_sorted(), "{groups:?}");
        for (i, g) in GROUPS.iter().enumerate() {
            assert!(groups.contains(&i), "nothing under {g}");
        }
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn typing_narrows_by_label_name_group_or_keys() {
        let found = |filter, user: &[Binding]| panel(filter).matches(user);
        let splits = [Action::SplitRight, Action::SplitDown];
        assert_eq!(found("split", &[]), splits);
        assert_eq!(found("Split_Right", &[]), [Action::SplitRight]);
        let tabs = found("tabs", &[]);
        assert!(tabs.contains(&Action::NewTab) && tabs.iter().all(|&a| group(a) == 0));
        // Ctrl+Shift+Tab has it in it too.
        let previous = Action::CycleTab(-1);
        assert_eq!(found("ctrl+shift+t", &[]), [Action::NewTab, previous]);
        let user = config::Config::parse("keybind = alt+f11=command_palette").keys;
        assert_eq!(found("alt+f11", &user), [Action::Palette]);
        assert!(found("alt+f11", &[]).is_empty());
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn rows_show_every_key_and_mark_changes() {
        let p = panel("copy");
        let rows = p.rows(&[]);
        let all = "Ctrl+C \u{b7} Ctrl+Shift+C \u{b7} Ctrl+Insert";
        assert_eq!(rows[0].label, "Copy");
        assert_eq!(
            (rows[0].value.as_str(), rows[0].default.as_str()),
            (all, all)
        );
        assert_eq!(rows[0].help, format!("{all}. In config.toml it is copy."));
        assert!(!rows[0].changed && rows[0].note.is_none());
        assert_eq!(rows[1].label, "Copy without indent");
        assert_eq!(
            (rows[1].value.as_str(), rows[1].default.as_str()),
            ("", "none")
        );
        let none = "No keys. In config.toml it is copy_without_indent.";
        assert_eq!(rows[1].help, none);
        let user = config::Config::parse("keybind = ctrl+shift+c=none").keys;
        let rows = p.rows(&user);
        assert_eq!(rows[0].value, "Ctrl+C \u{b7} Ctrl+Insert");
        assert!(rows[0].changed && !rows[1].changed);
        assert_eq!(rows[0].default, all);
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn waiting_shows_what_was_pressed_and_what_it_takes() {
        let user = config::Config::parse(r"keybind = ctrl+shift+g=text:git status\r").keys;
        let mut p = panel("split_right");
        p.wait(&user);
        let row = |p: &Panel| p.rows(&user).remove(0);
        let waiting = row(&p);
        assert_eq!((waiting.value.as_str(), waiting.note), ("press keys", None));
        let help = "Press the keys to add to Split right. Del gives its keys back to the \
                    program; Esc stops.";
        assert_eq!(waiting.help, help);
        for (keys, says) in [
            (
                "a",
                "A key alone goes to the program; hold Ctrl or Alt with it",
            ),
            (
                "shift+a",
                "Shift and this key go to the program; hold Ctrl or Alt too",
            ),
            (
                "ctrl+shift+t",
                "Ctrl+Shift+T runs New tab now; Enter moves it here",
            ),
            (
                "ctrl+shift+g",
                "Ctrl+Shift+G types text now; Enter makes it Split right instead",
            ),
            (
                "ctrl+shift+r",
                "Ctrl+Shift+R already runs Split right; Del gives it back to the program",
            ),
        ] {
            assert!(p.press(Some(chord(keys)), &user).is_empty());
            let r = row(&p);
            assert_eq!(r.note.as_deref(), Some(says), "{keys}");
            assert_eq!(r.value, keymap::chord_label(chord(keys).0, chord(keys).1));
        }
        p.press(Some(chord("ctrl+alt+y")), &user);
        assert_eq!(row(&p).note, None, "free keys");
        p.press(None, &user);
        let altgr = row(&p);
        assert_eq!(
            (altgr.value.as_str(), altgr.note.as_deref()),
            ("press keys", Some(ALTGR))
        );
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn enter_adds_keys_and_moves_them_from_another_action() {
        let mut c = config::Config::default();
        let mut p = panel("split_right");
        p.wait(&c.keys);
        assert!(p.press(Some(chord("ctrl+shift+t")), &c.keys).is_empty());
        let r = p.press(Some(chord("enter")), &c.keys);
        let line = Some("ctrl+shift+t=split_right".to_string());
        assert_eq!(r, [(chord("ctrl+shift+t"), line)]);
        assert!(p.capture.is_none());
        c.set_keys(&r);
        assert!(keymap::chords(Action::NewTab, &c.keys).is_empty());
        let both = [chord("ctrl+shift+t"), chord("ctrl+shift+r")];
        assert_eq!(keymap::chords(Action::SplitRight, &c.keys), both);
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_highlight_stays_on_its_action_as_keys_change() {
        let mut c = config::Config::default();
        let previous = Action::CycleTab(-1);
        let mut p = panel("ctrl+shift+t");
        p.move_by(1, &c.keys);
        assert_eq!(p.selected(&c.keys), Some(previous));
        p.wait(&c.keys);
        p.press(Some(chord("ctrl+shift+t")), &c.keys);
        let r = p.press(Some(chord("enter")), &c.keys);
        let old = c.keys.clone();
        c.set_keys(&r);
        p.rekey(&old, &c.keys);
        // New tab has no Ctrl+Shift+T now, so it left the list above.
        assert_eq!(p.matches(&c.keys), [previous]);
        assert_eq!(p.selected(&c.keys), Some(previous));
        // A config.toml that takes the waiting action out of the list
        // stops the wait.
        p.wait(&c.keys);
        let given = config::Config::parse(
            "keybind = ctrl+shift+t=none\n\
             keybind = ctrl+shift+tab=none\n",
        );
        p.rekey(&c.keys, &given.keys);
        assert!(p.capture.is_none() && p.matches(&given.keys).is_empty());
        assert_eq!(p.sel, 0);
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn enter_waits_on_keys_that_cannot_be_bound() {
        let mut p = panel("split_right");
        p.wait(&[]);
        let enter = Some(chord("enter"));
        assert!(p.press(enter, &[]).is_empty(), "nothing pressed yet");
        for keys in [Some(chord("shift+enter")), None] {
            p.press(keys, &[]);
            assert!(p.press(enter, &[]).is_empty());
            assert!(p.capture.is_some(), "still waiting");
        }
        // Its own keys: nothing to add.
        p.press(Some(chord("ctrl+shift+r")), &[]);
        assert!(p.press(enter, &[]).is_empty());
        assert!(p.capture.is_none());
    }

    #[test]
    fn esc_stops_waiting() {
        for esc in ["esc", "ctrl+esc", "shift+esc"] {
            let mut p = panel("");
            p.wait(&[]);
            p.press(Some(chord("ctrl+alt+y")), &[]);
            assert!(p.press(Some(chord(esc)), &[]).is_empty());
            assert!(p.capture.is_none(), "{esc}");
        }
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn del_gives_back_one_key_or_all() {
        let none = |keys: &str| (chord(keys), Some(format!("{keys}=none")));
        let mut p = panel("copy");
        p.wait(&[]);
        p.press(Some(chord("ctrl+c")), &[]);
        assert_eq!(p.press(Some(chord("del")), &[]), [none("ctrl+c")]);
        assert!(p.capture.is_none());
        p.wait(&[]);
        let all = [none("ctrl+c"), none("ctrl+shift+c"), none("ctrl+insert")];
        assert_eq!(p.press(Some(chord("backspace")), &[]), all);
        // Keys only a binding gives it have no line once given back.
        let user = config::Config::parse("keybind = alt+c=copy").keys;
        p.wait(&user);
        p.press(Some(chord("alt+c")), &user);
        assert_eq!(p.press(Some(chord("del")), &user), [(chord("alt+c"), None)]);
    }

    #[test]
    fn a_held_key_that_stops_waiting_does_nothing_more() {
        let mut p = panel("");
        p.wait(&[]);
        assert!(!p.repeats_stop(0x08, false));
        p.press(Some(chord("backspace")), &[]);
        assert!(p.capture.is_none());
        assert!(p.repeats_stop(0x08, true) && p.repeats_stop(0x08, true));
        // Pressed again, it erases the search, and so do its repeats.
        assert!(!p.repeats_stop(0x08, false));
        assert!(!p.repeats_stop(0x08, true));
        // Keys pressed while waiting are not held back.
        p.wait(&[]);
        p.press(Some(chord("ctrl+alt+y")), &[]);
        assert!(!p.repeats_stop(0x59, true));
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn del_in_the_list_puts_back_the_defaults() {
        let mut c = config::Config::parse(
            "keybind = ctrl+shift+c=none\n\
             keybind = alt+c=copy\n\
             keybind = ctrl+c=paste\n",
        );
        let r = reset(Action::Copy, &c.keys);
        let drop = ["alt+c", "ctrl+shift+c", "ctrl+c"].map(|k| (chord(k), None));
        assert_eq!(r.len(), 3);
        assert!(drop.iter().all(|d| r.contains(d)), "{r:?}");
        c.set_keys(&r);
        for a in [Action::Copy, Action::Paste] {
            assert_eq!(keymap::chords(a, &c.keys), keymap::defaults(a), "{a:?}");
        }
        assert!(reset(Action::Copy, &c.keys).is_empty());
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn adding_a_default_back_drops_the_line() {
        let mut c = config::Config::parse("keybind = ctrl+shift+t=split_right");
        let mut p = panel("new_tab");
        p.wait(&c.keys);
        p.press(Some(chord("ctrl+shift+t")), &c.keys);
        let r = p.press(Some(chord("enter")), &c.keys);
        assert_eq!(r, [(chord("ctrl+shift+t"), None)]);
        c.set_keys(&r);
        assert!(c.keys.is_empty());
        let t = [chord("ctrl+shift+t")];
        assert_eq!(keymap::chords(Action::NewTab, &c.keys), t);
    }
}
