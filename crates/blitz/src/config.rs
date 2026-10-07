//! Settings: built-in defaults, with the ones in [`SETTINGS`] read from
//! `%APPDATA%\blitz\config.toml`.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub font_family: String,
    /// Used when `font_family` is not installed.
    pub font_fallback: String,
    /// Points; pixels = pt * dpi / 72.
    pub font_size: f32,
    pub line_height: f32,
    /// A theme name, or `light:NAME,dark:NAME` to follow the Windows app
    /// theme; see [`crate::theme::choose`].
    pub theme: String,
    pub accent: Option<u32>,
    /// Empty means detect: pwsh, then Windows PowerShell, then cmd.
    pub shell: String,
    pub shell_args: Vec<String>,
    pub shell_integration: bool,
    pub scrollback_lines: usize,
    /// Flash the taskbar button when a session needs attention.
    pub flash: bool,
    /// Whether BEL in an unfocused pane asks for attention.
    pub bell_attention: bool,
    pub command_finish_after_ms: u64,
    /// Look for a newer release on GitHub at start and once a day.
    pub check_updates: bool,
    /// Reopen the last window's tabs, splits and folders at start.
    pub restore_session: bool,
    /// Reopen the Claude Code session a restored pane was running.
    pub restore_claude: bool,
    /// Save each pane's recent output and show it again on the next start.
    /// Off by default: old output can hold secrets.
    pub restore_scrollback: bool,
    /// Pixel scenery behind the panes, one of
    /// [`SCENES`](crate::arcade::scenery::SCENES).
    pub scenery: String,
    /// The spark at the foot of the sidebar.
    pub mascot: bool,
    /// Overrides as (chord, action) pairs, e.g. ("ctrl+shift+r", "split_right").
    pub keys: Vec<(String, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_family: "Cascadia Mono".into(),
            font_fallback: "Consolas".into(),
            font_size: 11.0,
            line_height: 1.0,
            theme: crate::theme::DEFAULT.into(),
            accent: None,
            shell: String::new(),
            shell_args: Vec::new(),
            shell_integration: true,
            scrollback_lines: 10_000,
            flash: true,
            bell_attention: true,
            command_finish_after_ms: 5000,
            check_updates: true,
            restore_session: true,
            restore_claude: true,
            restore_scrollback: false,
            scenery: "off".into(),
            mascot: false,
            keys: vec![("ctrl+shift+r".into(), "split_right".into())],
        }
    }
}

/// How the settings panel changes a setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// On or off.
    Toggle,
    /// One of a list: presets, or what is installed.
    Choice,
    /// Picked in the theme picker.
    Theme,
    /// Starts blitz run; not a value in `config.toml`.
    Game,
}

/// A setting the settings panel shows. `key` is its name in `config.toml`.
#[derive(Debug, PartialEq, Eq)]
pub struct Setting {
    pub key: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// When a change takes effect.
    pub applies: &'static str,
}

const NOW: &str = "Applies now";
const NEW_PANES: &str = "Applies to new panes";
const RESTART: &str = "Applies when blitz restarts";

/// Every setting the panel shows, in its order.
pub const SETTINGS: &[Setting] = &[
    Setting {
        key: "theme",
        group: "Appearance",
        label: "Theme",
        help: "Colours for the panes, sidebar and window frame. Enter opens the theme picker.",
        kind: Kind::Theme,
        applies: NOW,
    },
    Setting {
        key: "font_family",
        group: "Appearance",
        label: "Font",
        help: "Lists the fixed-width fonts installed. If the font is removed, \
               blitz falls back to Cascadia Mono, then Consolas.",
        kind: Kind::Choice,
        applies: NOW,
    },
    Setting {
        key: "font_size",
        group: "Appearance",
        label: "Font size",
        help: "In points. Windows display scaling applies on top.",
        kind: Kind::Choice,
        applies: NOW,
    },
    Setting {
        key: "restore_session",
        group: "Sessions",
        label: "Reopen tabs",
        help: "Reopen the last window's tabs, splits and folders when blitz starts.",
        kind: Kind::Toggle,
        applies: RESTART,
    },
    Setting {
        key: "restore_claude",
        group: "Sessions",
        label: "Resume Claude Code",
        help: "Resume the Claude Code session each reopened pane was running. \
               Needs the hooks from blitz setup claude.",
        kind: Kind::Toggle,
        applies: RESTART,
    },
    Setting {
        key: "restore_scrollback",
        group: "Sessions",
        label: "Restore output",
        help: "Save each pane's last 1000 lines when blitz closes and show them \
               again on the next start. Old output can hold secrets.",
        kind: Kind::Toggle,
        applies: RESTART,
    },
    Setting {
        key: "shell",
        group: "Shell",
        label: "Shell",
        help: "The program each new pane runs. Automatic picks PowerShell 7, \
               then Windows PowerShell, then cmd.",
        kind: Kind::Choice,
        applies: NEW_PANES,
    },
    Setting {
        key: "shell_integration",
        group: "Shell",
        label: "Shell integration",
        help: "Lets PowerShell and cmd tell blitz their folder and where each \
               prompt starts, for the sidebar, new panes and reopened sessions.",
        kind: Kind::Toggle,
        applies: NEW_PANES,
    },
    Setting {
        key: "scrollback_lines",
        group: "Shell",
        label: "Scrollback",
        help: "Lines of history each pane keeps.",
        kind: Kind::Choice,
        applies: NEW_PANES,
    },
    Setting {
        key: "flash",
        group: "Notifications",
        label: "Flash taskbar",
        help: "Flash the taskbar button when a session needs you while blitz \
               is in the background.",
        kind: Kind::Toggle,
        applies: NOW,
    },
    Setting {
        key: "bell_attention",
        group: "Notifications",
        label: "Bell needs you",
        help: "A bell in a pane you are not looking at marks its session as \
               needing you, as a question from Claude Code does.",
        kind: Kind::Toggle,
        applies: NOW,
    },
    Setting {
        key: "check_updates",
        group: "Updates",
        label: "Check for updates",
        help: "Ask GitHub for a newer release at start and every six hours.",
        kind: Kind::Toggle,
        applies: RESTART,
    },
    Setting {
        key: "scenery",
        group: "Easter eggs",
        label: "Scenery",
        help: "Pixel stars, hills or snow drifting behind the panes, faint                enough to read over. Still when Windows animations are off.",
        kind: Kind::Choice,
        applies: NOW,
    },
    Setting {
        key: "mascot",
        group: "Easter eggs",
        label: "Spark",
        help: "A little critter at the foot of the sidebar that naps, runs                and waves along with your sessions.",
        kind: Kind::Toggle,
        applies: NOW,
    },
    Setting {
        key: "game",
        group: "Easter eggs",
        label: "blitz run",
        help: "A one-button runner for while agents work. Enter starts it,                Space jumps, Esc quits; it closes when a session needs you.",
        kind: Kind::Game,
        applies: NOW,
    },
];

impl Config {
    /// The defaults with the settings from a `config.toml` applied: one
    /// `key = value` per line, `#` starts a comment. Lines it doesn't
    /// understand are skipped, so a typo never stops blitz from starting.
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        // Notepad may save with a byte order mark.
        for line in text.trim_start_matches('\u{feff}').lines() {
            if let Some((key, value)) = entry(line) {
                c.set(key, value);
            }
        }
        c
    }

    /// Setting `key` as `config.toml` writes it: `true`, `11` or
    /// `"Consolas"`. Empty for an unknown key.
    pub fn get(&self, key: &str) -> String {
        let flag = |b: bool| b.to_string();
        match key {
            "theme" => quote(&self.theme),
            "font_family" => quote(&self.font_family),
            "font_size" => self.font_size.to_string(),
            "shell" => quote(&self.shell),
            "shell_integration" => flag(self.shell_integration),
            "scrollback_lines" => self.scrollback_lines.to_string(),
            "restore_session" => flag(self.restore_session),
            "restore_claude" => flag(self.restore_claude),
            "restore_scrollback" => flag(self.restore_scrollback),
            "flash" => flag(self.flash),
            "bell_attention" => flag(self.bell_attention),
            "check_updates" => flag(self.check_updates),
            "scenery" => quote(&self.scenery),
            "mascot" => flag(self.mascot),
            _ => String::new(),
        }
    }

    /// Sets `key` from a value as `config.toml` holds it. Returns false,
    /// changing nothing, for an unknown key or a value of the wrong type or
    /// out of range. On/off and numbers must not be quoted; names may be.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let text = unquote(value);
        let bare = text.is_none().then_some(value);
        let num = bare.and_then(|v| v.parse::<f64>().ok());
        let text = text.unwrap_or(value).to_string();
        match key {
            "theme" if !text.is_empty() => self.theme = text,
            "font_family" => self.font_family = text,
            "shell" => self.shell = text,
            "scenery" => match text.to_lowercase() {
                s if crate::arcade::scenery::SCENES.contains(&s.as_str()) => self.scenery = s,
                _ => return false,
            },
            "font_size" => match num.filter(|n| (4.0..=72.0).contains(n)) {
                Some(n) => self.font_size = n as f32,
                None => return false,
            },
            "scrollback_lines" => match num.filter(|n| (0.0..=100_000.0).contains(n)) {
                Some(n) if n.fract() == 0.0 => self.scrollback_lines = n as usize,
                _ => return false,
            },
            _ => match (self.flag(key), bare.and_then(|v| v.parse().ok())) {
                (Some(field), Some(on)) => *field = on,
                _ => return false,
            },
        }
        true
    }

    fn flag(&mut self, key: &str) -> Option<&mut bool> {
        Some(match key {
            "shell_integration" => &mut self.shell_integration,
            "restore_session" => &mut self.restore_session,
            "restore_claude" => &mut self.restore_claude,
            "restore_scrollback" => &mut self.restore_scrollback,
            "flash" => &mut self.flash,
            "bell_attention" => &mut self.bell_attention,
            "check_updates" => &mut self.check_updates,
            "mascot" => &mut self.mascot,
            _ => return None,
        })
    }

    /// Reads `%APPDATA%\blitz\config.toml`, or gives the defaults.
    pub fn load() -> Config {
        dir().map_or_else(Config::default, |d| Config::read(&d.join(FILE)))
    }

    /// Reads `config.toml` again while blitz runs. `None` when the file is
    /// there but cannot be read now, as while an editor saves it.
    pub fn reload() -> Option<Config> {
        let Some(d) = dir() else {
            return Some(Config::default());
        };
        match std::fs::read_to_string(d.join(FILE)) {
            Ok(text) => Some(Config::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Config::default()),
            Err(_) => None,
        }
    }

    /// A missing or unreadable file gives the defaults.
    fn read(path: &Path) -> Config {
        Config::parse(&std::fs::read_to_string(path).unwrap_or_default())
    }
}

/// The key and value of a `key = value` line, without the comment after
/// it. A quoted value keeps its quotes and may hold a `#`.
fn entry(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = line.split_once('=')?;
    let rest = rest.trim_start();
    let value = match rest.chars().next() {
        Some(q @ ('"' | '\'')) => &rest[..rest[1..].find(q)? + 2],
        _ => rest.split('#').next().unwrap_or_default().trim(),
    };
    Some((key.trim(), value))
}

/// The text of a quoted value; `None` when it is not quoted.
fn unquote(value: &str) -> Option<&str> {
    let q = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    value.strip_prefix(q)?.strip_suffix(q)
}

/// `s` as a TOML string. One holding `"` or `\` is a literal string, as a
/// basic one would read those as escapes.
pub fn quote(s: &str) -> String {
    if s.contains(['"', '\\']) && !s.contains('\'') {
        format!("'{s}'")
    } else {
        format!("\"{s}\"")
    }
}

const FILE: &str = "config.toml";

/// `%APPDATA%\blitz`, which holds `config.toml` and the themes folder.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("blitz"))
}

/// Sets `key = value` in `config.toml`, keeping the rest of the file; with
/// no value, removes `key` so its default applies. Writes a new file and
/// moves it over the old one, so a failed save never leaves half a file.
pub fn save(key: &str, value: Option<&str>) -> std::io::Result<()> {
    let dir = dir().ok_or_else(|| std::io::Error::other("no APPDATA"))?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(FILE);
    let old = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        r => r?,
    };
    let tmp = dir.join("config.toml.new");
    std::fs::write(&tmp, with_value(&old, key, value))?;
    std::fs::rename(tmp, path)
}

/// `text` with its last `key` line, the one that counts, set to `value`,
/// or one added at the end. With no value, every `key` line goes.
fn with_value(text: &str, key: &str, value: Option<&str>) -> String {
    let bom = if text.starts_with('\u{feff}') {
        "\u{feff}"
    } else {
        ""
    };
    let mut lines: Vec<String> = (text.trim_start_matches('\u{feff}').lines())
        .map(str::to_string)
        .collect();
    let is_key = |l: &String| entry(l).is_some_and(|(k, _)| k == key);
    match value {
        Some(v) => {
            let line = format!("{key} = {v}");
            match lines.iter().rposition(is_key) {
                Some(i) => lines[i] = line,
                None => lines.push(line),
            }
        }
        None => lines.retain(|l| !is_key(l)),
    }
    if lines.is_empty() {
        return bom.to_string();
    }
    format!("{bom}{}\n", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_reads_bools() {
        let c = Config::parse(
            "\u{feff}# blitz settings\n\
             restore_scrollback = true\n\
             \tflash=false   # no flashing\n\
             \n\
             check_updates =  false\r\n",
        );
        assert!(c.restore_scrollback);
        assert!(!c.flash);
        assert!(!c.check_updates);
        assert!(c.bell_attention);
    }

    #[test]
    fn config_skips_what_it_does_not_know() {
        let c = Config::parse(
            "font_size = 300\n\
             font_size = \"12\"\n\
             scrollback_lines = 1.5\n\
             flash = no\n\
             bell_attention = \"false\"\n\
             restore_scrollback\n\
             = true\n\
             # check_updates = false\n\
             theme = \"unterminated\n\
             [section]\n",
        );
        assert_eq!(c, Config::default());
        assert_eq!(Config::parse(""), Config::default());
    }

    #[test]
    fn config_reads_theme() {
        assert_eq!(Config::parse("theme = \"Rose Pine\"\n").theme, "Rose Pine");
        assert_eq!(
            Config::parse("theme=Tokyo Night # mine").theme,
            "Tokyo Night"
        );
        assert_eq!(Config::parse("theme = \"\"").theme, crate::theme::DEFAULT);
        let c = Config::parse("theme = \"No #1\" # mine\n# theme = x");
        assert_eq!(c.theme, "No #1");
    }

    #[test]
    fn config_reads_fonts_shell_and_numbers() {
        let c = Config::parse(
            "font_family = \"JetBrains Mono\"\n\
             font_size = 13.5\n\
             shell = 'C:\\Program Files\\Git\\bin\\bash.exe' # git\n\
             scrollback_lines = 50000\n\
             shell_integration = false\n",
        );
        assert_eq!(c.font_family, "JetBrains Mono");
        assert_eq!(c.font_size, 13.5);
        assert_eq!(c.shell, r"C:\Program Files\Git\bin\bash.exe");
        assert_eq!(c.scrollback_lines, 50_000);
        assert!(!c.shell_integration);
        // An empty shell is the automatic one.
        assert_eq!(Config::parse("shell = \"cmd\"\nshell = \"\"").shell, "");
    }

    #[test]
    fn every_setting_reads_back_what_it_writes() {
        let mut c = Config::default();
        for s in SETTINGS.iter().filter(|s| s.kind != Kind::Game) {
            let v = c.get(s.key);
            assert!(!v.is_empty(), "{} has no value", s.key);
            assert!(c.set(s.key, &v), "{} = {v} does not read back", s.key);
        }
        assert_eq!(c, Config::default());
        c.shell = r#"C:\x "y"\sh.exe"#.into();
        let v = c.get("shell");
        assert!(Config::default().set("shell", &v));
        assert_eq!(Config::parse(&format!("shell = {v}")).shell, c.shell);
    }

    #[test]
    fn saving_a_value_keeps_the_rest_of_the_file() {
        let theme = |text: &str, name: &str| with_value(text, "theme", Some(&quote(name)));
        assert_eq!(theme("", "A"), "theme = \"A\"\n");
        assert_eq!(
            theme("flash = false\r\n# theme = old\n", "A"),
            "flash = false\n# theme = old\ntheme = \"A\"\n"
        );
        let saved = theme("\u{feff}theme = old\nflash = false", "B C");
        assert_eq!(saved, "\u{feff}theme = \"B C\"\nflash = false\n");
        let saved = theme("theme = a\ntheme = b # mine\n", "C");
        assert_eq!(Config::parse(&saved).theme, "C", "the last line counts");
    }

    #[test]
    fn saving_no_value_removes_the_key() {
        let text = "# mine\nflash = false\nfont_size = 14\nflash = true # again\n";
        assert_eq!(
            with_value(text, "flash", None),
            "# mine\nfont_size = 14\n",
            "comments and other keys stay"
        );
        assert_eq!(with_value("flash = false\n", "flash", None), "");
        assert_eq!(
            with_value("\u{feff}flash = false", "flash", None),
            "\u{feff}"
        );
    }

    #[test]
    fn config_reads_easter_eggs() {
        let c = Config::parse(
            "scenery = \"Hills\"
mascot = true
",
        );
        assert_eq!((c.scenery.as_str(), c.mascot), ("hills", true));
        let c = Config::parse(
            "scenery = \"lava\"
scenery = stars
",
        );
        assert_eq!(c.scenery, "stars", "an unknown scene is skipped");
        assert_eq!(Config::default().scenery, "off");
    }

    #[test]
    fn config_missing_file_gives_defaults() {
        let path = std::env::temp_dir().join("blitz-no-such-config.toml");
        assert_eq!(Config::read(&path), Config::default());
    }
}
