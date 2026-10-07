//! The settings panel: which settings match what was typed, what each one
//! shows, and the value the arrow keys move it to.

use crate::config::{Config, Kind, SETTINGS, Setting, TOASTS, quote};
use crate::render::chrome::SettingRow;

/// Font sizes offered, in points.
const FONT_SIZES: &[f32] = &[
    8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0,
];
/// Line heights offered, as multiples of the font's own.
const LINE_HEIGHTS: &[f32] = &[0.8, 0.9, 1.0, 1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.8, 2.0];
/// Scrollback lengths offered.
const SCROLLBACK: &[usize] = &[1000, 5000, 10_000, 20_000, 50_000, 100_000];
/// Editors offered for Ctrl+click on a path, by the URI that opens a file
/// at a line. Others can be set in `config.toml`.
const EDITORS: &[(&str, &str)] = &[
    ("Off", ""),
    ("VS Code", "vscode://file/{path}:{line}:{col}"),
    ("Cursor", "cursor://file/{path}:{line}:{col}"),
];

/// The open settings panel.
#[derive(Debug, Default)]
pub struct Panel {
    /// Typed text; settings that contain each typed word match.
    pub filter: String,
    /// The highlighted setting, among the matching ones.
    pub sel: usize,
    /// The first list line shown in the last frame, so the list scrolls
    /// only when the highlight leaves it.
    pub top: usize,
    /// Fixed-width fonts installed, sorted.
    pub fonts: Vec<String>,
    /// Shells to choose from, as (name, path); an empty path is automatic.
    pub shells: Vec<(String, String)>,
    /// Why the last change was not saved.
    pub error: Option<String>,
}

impl Panel {
    pub fn new(fonts: Vec<String>, shells: Vec<(String, String)>) -> Panel {
        Panel {
            fonts,
            shells,
            ..Panel::default()
        }
    }

    /// The settings that match the filter, in panel order. The filter's
    /// words are looked for in each setting's group, label, key and help.
    pub fn matches(&self) -> Vec<&'static Setting> {
        let words: Vec<String> = (self.filter.split_whitespace())
            .map(str::to_lowercase)
            .collect();
        (SETTINGS.iter())
            .filter(|s| {
                let text = format!("{} {} {} {}", s.group, s.label, s.key, s.help).to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect()
    }

    pub fn selected(&self) -> Option<&'static Setting> {
        self.matches().get(self.sel).copied()
    }

    /// Moves the highlight `by` rows, stopping at either end.
    pub fn move_by(&mut self, by: isize) {
        let last = self.matches().len().saturating_sub(1);
        self.sel = self.sel.saturating_add_signed(by).min(last);
    }

    /// The values a choice can take, as (shown, as written), in order.
    /// A value from `config.toml` that is not among them is added, so the
    /// panel can always show what is set.
    fn choices(&self, s: &Setting, c: &Config) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = match s.key {
            "font_family" => (self.fonts.iter()).map(|f| (f.clone(), quote(f))).collect(),
            "font_size" => (FONT_SIZES.iter())
                .map(|n| (format!("{n} pt"), n.to_string()))
                .collect(),
            "line_height" => (LINE_HEIGHTS.iter())
                .map(|n| (format!("{n:.1}"), n.to_string()))
                .collect(),
            "scrollback_lines" => (SCROLLBACK.iter())
                .map(|&n| (format!("{} lines", thousands(n)), n.to_string()))
                .collect(),
            "scenery" => (crate::arcade::scenery::SCENES.iter())
                .map(|v| (title(v), quote(v)))
                .collect(),
            "toasts" => (TOASTS.iter())
                .map(|v| (title(&v.replace('-', " ")), quote(v)))
                .collect(),
            "shell" => (self.shells.iter())
                .map(|(name, path)| (name.clone(), quote(path)))
                .collect(),
            "editor_uri" => (EDITORS.iter())
                .map(|(name, uri)| (name.to_string(), quote(uri)))
                .collect(),
            _ => Vec::new(),
        };
        let now = c.get(s.key);
        if !out.iter().any(|(_, v)| v.eq_ignore_ascii_case(&now)) {
            let shown = match s.key {
                "font_size" => format!("{} pt", c.font_size),
                "scrollback_lines" => format!("{} lines", thousands(c.scrollback_lines)),
                "shell" => c
                    .shell
                    .rsplit(['\\', '/'])
                    .next()
                    .unwrap_or_default()
                    .into(),
                _ => c.get(s.key).trim_matches(['"', '\'']).into(),
            };
            out.push((shown, now));
            // Numbers stay in order; names go at the end.
            out.sort_by(|a, b| {
                let n = |v: &str| v.parse::<f64>().unwrap_or(f64::MAX);
                n(&a.1).total_cmp(&n(&b.1))
            });
        }
        out
    }

    /// What `s` shows for its value in `c`.
    pub fn shown(&self, s: &Setting, c: &Config) -> String {
        match s.kind {
            Kind::Toggle => if c.get(s.key) == "true" { "on" } else { "off" }.into(),
            Kind::Theme => {
                crate::theme::choose(&c.theme, crate::theme::system_is_light()).to_string()
            }
            Kind::Game => "play".into(),
            Kind::Choice => {
                let now = c.get(s.key);
                (self.choices(s, c).into_iter())
                    .find(|(_, v)| v.eq_ignore_ascii_case(&now))
                    .map(|(shown, _)| shown)
                    .unwrap_or_default()
            }
        }
    }

    /// The value of `s` as written, `by` steps on from the one in `c`:
    /// on for a step right and off for a step left, or the choice that
    /// many places on. With `wrap`, a toggle flips and a choice goes round
    /// from the last to the first. `None` when nothing would change.
    pub fn step(&self, s: &Setting, c: &Config, by: isize, wrap: bool) -> Option<String> {
        let now = c.get(s.key);
        let next = match s.kind {
            Kind::Toggle if wrap => (now != "true").to_string(),
            Kind::Toggle => (by > 0).to_string(),
            Kind::Theme | Kind::Game => return None,
            Kind::Choice => {
                let list = self.choices(s, c);
                let i = list
                    .iter()
                    .position(|(_, v)| v.eq_ignore_ascii_case(&now))?;
                let n = list.len() as isize;
                let j = if wrap {
                    (i as isize + by).rem_euclid(n)
                } else {
                    (i as isize + by).clamp(0, n - 1)
                };
                list[j as usize].1.clone()
            }
        };
        (next != now).then_some(next)
    }

    /// The rows the chrome draws: each matching setting with its value.
    /// Checking for updates also says why the last look or update failed.
    pub fn rows(&self, c: &Config, update_error: Option<&str>) -> Vec<SettingRow> {
        let d = Config::default();
        (self.matches().into_iter())
            .map(|s| SettingRow {
                note: update_error
                    .filter(|_| s.key == "check_updates")
                    .map(Into::into),
                group: s.group,
                label: s.label,
                help: s.help,
                applies: s.applies,
                on: (s.kind == Kind::Toggle).then(|| c.get(s.key) == "true"),
                value: self.shown(s, c),
                less: s.kind == Kind::Theme || self.step(s, c, -1, false).is_some(),
                more: s.kind == Kind::Theme || self.step(s, c, 1, false).is_some(),
                default: self.shown(s, &d),
                changed: c.get(s.key) != d.get(s.key),
            })
            .collect()
    }
}

/// `stars` as `Stars`.
fn title(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// `10000` as `10,000`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Panel {
        Panel::new(
            vec!["Cascadia Mono".into(), "Consolas".into()],
            vec![
                ("Automatic (PowerShell 7)".into(), String::new()),
                (
                    "Command Prompt".into(),
                    r"C:\Windows\System32\cmd.exe".into(),
                ),
            ],
        )
    }

    fn setting(key: &str) -> &'static Setting {
        SETTINGS.iter().find(|s| s.key == key).expect(key)
    }

    #[test]
    fn typing_narrows_the_list_by_every_word() {
        let mut p = panel();
        assert_eq!(p.matches().len(), SETTINGS.len());
        p.filter = "FONT".into();
        let keys: Vec<_> = p.matches().iter().map(|s| s.key).collect();
        assert_eq!(keys, ["font_family", "font_size", "line_height"]);
        // Help text counts: the scrollback warning mentions secrets.
        p.filter = "secrets".into();
        assert_eq!(p.selected().map(|s| s.key), Some("restore_scrollback"));
        p.filter = "font size".into();
        assert_eq!(p.matches().len(), 1);
        p.filter = "zzz".into();
        assert!(p.selected().is_none());
    }

    #[test]
    fn the_highlight_stops_at_either_end() {
        let mut p = panel();
        p.move_by(-1);
        assert_eq!(p.sel, 0);
        p.move_by(100);
        assert_eq!(p.sel, SETTINGS.len() - 1);
    }

    #[test]
    fn toggles_go_on_to_the_right_and_flip_with_enter() {
        let (p, c) = (panel(), Config::default());
        let flash = setting("flash");
        assert_eq!(p.step(flash, &c, 1, false), None, "already on");
        assert_eq!(p.step(flash, &c, -1, false).as_deref(), Some("false"));
        assert_eq!(p.step(flash, &c, 1, true).as_deref(), Some("false"));
        assert_eq!(p.shown(flash, &c), "on");
    }

    #[test]
    fn choices_step_in_order_and_stop_or_wrap_at_the_ends() {
        let (p, mut c) = (panel(), Config::default());
        let size = setting("font_size");
        assert_eq!(p.step(size, &c, 1, false).as_deref(), Some("12"));
        assert_eq!(p.step(size, &c, -1, false).as_deref(), Some("10"));
        c.font_size = 32.0;
        assert_eq!(p.step(size, &c, 1, false), None);
        assert_eq!(p.step(size, &c, 1, true).as_deref(), Some("8"));
        // A size set by hand sits in order among the presets.
        c.font_size = 11.5;
        assert_eq!(p.shown(size, &c), "11.5 pt");
        assert_eq!(p.step(size, &c, 1, false).as_deref(), Some("12"));
        assert_eq!(p.step(size, &c, -1, false).as_deref(), Some("11"));
    }

    #[test]
    fn line_heights_step_through_their_presets() {
        let (p, mut c) = (panel(), Config::default());
        let lh = setting("line_height");
        assert_eq!(p.shown(lh, &c), "1.0");
        assert_eq!(p.step(lh, &c, 1, false).as_deref(), Some("1.1"));
        assert_eq!(p.step(lh, &c, -1, false).as_deref(), Some("0.9"));
        c.line_height = 2.0;
        assert_eq!(p.step(lh, &c, 1, false), None);
        // A height set by hand shows as written.
        c.line_height = 1.25;
        assert_eq!(p.shown(lh, &c), "1.25");
        assert_eq!(p.step(lh, &c, 1, false).as_deref(), Some("1.3"));
    }

    #[test]
    fn shells_and_fonts_show_names() {
        let (p, mut c) = (panel(), Config::default());
        let shell = setting("shell");
        assert_eq!(p.shown(shell, &c), "Automatic (PowerShell 7)");
        let next = p.step(shell, &c, 1, false).expect("a next shell");
        assert!(c.set("shell", &next));
        assert_eq!(c.shell, r"C:\Windows\System32\cmd.exe");
        assert_eq!(p.shown(shell, &c), "Command Prompt");
        // A shell set by hand shows its file name.
        c.shell = r"D:\tools\nu.exe".into();
        assert_eq!(p.shown(shell, &c), "nu.exe");
        // Font names match whatever their case in config.toml.
        c.font_family = "consolas".into();
        assert_eq!(p.shown(setting("font_family"), &c), "Consolas");
        assert_eq!(thousands(100_000), "100,000");
        assert_eq!(thousands(999), "999");
    }

    #[test]
    fn values_set_by_hand_still_show_and_step() {
        let (p, mut c) = (panel(), Config::default());
        // A font that is not installed sits after the installed ones.
        c.font_family = "Fira Code".into();
        let font = setting("font_family");
        assert_eq!(p.shown(font, &c), "Fira Code");
        assert_eq!(p.step(font, &c, -1, false).as_deref(), Some("\"Consolas\""));
        assert_eq!(p.step(font, &c, 1, false), None);
        assert_eq!(
            p.step(font, &c, 1, true).as_deref(),
            Some("\"Cascadia Mono\"")
        );
        // A length between two presets sits between them.
        c.scrollback_lines = 1234;
        let lines = setting("scrollback_lines");
        assert_eq!(p.shown(lines, &c), "1,234 lines");
        assert_eq!(p.step(lines, &c, -1, false).as_deref(), Some("1000"));
        assert_eq!(p.step(lines, &c, 1, false).as_deref(), Some("5000"));
        // The theme moves only in the theme picker.
        let theme = setting("theme");
        assert_eq!(p.step(theme, &c, 1, false), None);
        assert_eq!(p.step(theme, &c, 1, true), None);
    }

    #[test]
    fn the_highlight_comes_back_into_a_shorter_list() {
        let mut p = panel();
        p.sel = SETTINGS.len() - 1;
        p.filter = "font".into();
        assert_eq!(p.selected(), None, "past the end of the matches");
        p.move_by(-1);
        assert_eq!(p.selected().map(|s| s.key), Some("line_height"));
        p.filter = "zzz".into();
        p.move_by(1);
        assert_eq!((p.sel, p.selected()), (0, None));
    }

    #[test]
    fn easter_eggs_step_through_scenes_and_offer_the_game() {
        let (p, mut c) = (panel(), Config::default());
        let scenery = setting("scenery");
        assert_eq!(p.shown(scenery, &c), "Off");
        assert_eq!(p.step(scenery, &c, -1, false), None);
        let next = p.step(scenery, &c, 1, false).expect("a scene");
        assert!(c.set("scenery", &next));
        assert_eq!(p.shown(scenery, &c), "Stars");
        let game = setting("game");
        assert_eq!(p.shown(game, &c), "play");
        assert_eq!(p.step(game, &c, 1, true), None);
    }

    #[test]
    fn editors_step_from_off_and_show_one_set_by_hand() {
        let (p, mut c) = (panel(), Config::default());
        let editor = setting("editor_uri");
        assert_eq!(p.shown(editor, &c), "Off");
        let next = p.step(editor, &c, 1, false).expect("an editor");
        assert!(c.set("editor_uri", &next));
        assert_eq!(c.editor_uri, "vscode://file/{path}:{line}:{col}");
        assert_eq!(p.shown(editor, &c), "VS Code");
        c.editor_uri = "zed://file/{path}:{line}".into();
        assert_eq!(p.shown(editor, &c), "zed://file/{path}:{line}");
        assert_eq!(
            p.step(editor, &c, -1, false).as_deref(),
            Some("\"cursor://file/{path}:{line}:{col}\"")
        );
    }

    #[test]
    fn notifications_step_from_none_to_all() {
        let (p, mut c) = (panel(), Config::default());
        let toasts = setting("toasts");
        assert_eq!(p.shown(toasts, &c), "Needs you");
        assert_eq!(p.step(toasts, &c, -1, false).as_deref(), Some("\"off\""));
        let next = p.step(toasts, &c, 1, false).expect("all");
        assert!(c.set("toasts", &next));
        assert_eq!(p.shown(toasts, &c), "All");
        assert_eq!(p.step(toasts, &c, 1, false), None);
    }

    #[test]
    fn rows_mark_changed_settings_and_where_they_can_move() {
        let p = panel();
        let c = Config {
            restore_scrollback: true,
            ..Config::default()
        };
        let rows = p.rows(&c, None);
        assert_eq!(rows.len(), SETTINGS.len());
        assert!(rows.iter().all(|r| r.note.is_none()));
        let row = |label| rows.iter().find(|r| r.label == label).expect(label);
        let out = row("Restore output");
        assert!(out.changed && out.on == Some(true) && out.default == "off");
        let size = row("Font size");
        assert!(!size.changed && size.on.is_none() && size.value == "11 pt");
        assert!(size.less && size.more);
        let font = row("Font");
        assert!(!font.less && font.more, "Cascadia Mono comes first");
    }

    #[test]
    fn checking_for_updates_says_why_the_last_look_failed() {
        let p = panel();
        let why = "Could not look for an update: curl: (7) Failed to connect";
        let rows = p.rows(&Config::default(), Some(why));
        let noted: Vec<_> = (rows.iter())
            .filter_map(|r| Some((r.label, r.note.as_deref()?)))
            .collect();
        assert_eq!(noted, [("Check for updates", why)]);
    }
}
