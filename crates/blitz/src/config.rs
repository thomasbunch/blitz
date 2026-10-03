//! Settings: built-in defaults, with a few read from
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
            keys: vec![("ctrl+shift+r".into(), "split_right".into())],
        }
    }
}

impl Config {
    /// The defaults with the settings from a `config.toml` applied: one
    /// `key = value` per line, `#` starts a comment. Lines it doesn't
    /// understand are skipped, so a typo never stops blitz from starting.
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        // Notepad may save with a byte order mark.
        for line in text.trim_start_matches('\u{feff}').lines() {
            // A quoted theme name may hold a `#`.
            if let Some((key, value)) = line.split_once('=')
                && key.trim() == "theme"
            {
                let value = value.trim_start();
                let name = match value.strip_prefix('"') {
                    Some(quoted) => quoted.split('"').next(),
                    None => value.split('#').next(),
                };
                let name = name.unwrap_or_default().trim();
                if !name.is_empty() {
                    c.theme = name.into();
                }
                continue;
            }
            let line = line.split('#').next().unwrap_or_default();
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = match value.trim() {
                "true" => true,
                "false" => false,
                _ => continue,
            };
            let field = match key.trim() {
                "restore_session" => &mut c.restore_session,
                "restore_claude" => &mut c.restore_claude,
                "restore_scrollback" => &mut c.restore_scrollback,
                "check_updates" => &mut c.check_updates,
                "flash" => &mut c.flash,
                "bell_attention" => &mut c.bell_attention,
                _ => continue,
            };
            *field = value;
        }
        c
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

const FILE: &str = "config.toml";

/// `%APPDATA%\blitz`, which holds `config.toml` and the themes folder.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("blitz"))
}

/// Sets `theme = "NAME"` in `config.toml`, keeping the rest of the file.
/// Writes a new file and moves it over the old one, so a failed save
/// never leaves half a file.
pub fn save_theme(name: &str) -> std::io::Result<()> {
    let dir = dir().ok_or_else(|| std::io::Error::other("no APPDATA"))?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(FILE);
    let old = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        r => r?,
    };
    let tmp = dir.join("config.toml.new");
    std::fs::write(&tmp, with_theme(&old, name))?;
    std::fs::rename(tmp, path)
}

/// `text` with its last `theme` line, the one that counts, set to `name`,
/// or one added at the end.
fn with_theme(text: &str, name: &str) -> String {
    let line = format!("theme = \"{name}\"");
    let is_theme = |l: &String| {
        let code = l.trim_start_matches('\u{feff}').split('#').next();
        code.and_then(|c| c.split_once('='))
            .is_some_and(|(k, _)| k.trim() == "theme")
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    match lines.iter().rposition(is_theme) {
        Some(i) => {
            // Keep a byte order mark on the first line.
            let bom = if lines[i].starts_with('\u{feff}') {
                "\u{feff}"
            } else {
                ""
            };
            lines[i] = format!("{bom}{line}");
        }
        None => lines.push(line),
    }
    lines.join("\n") + "\n"
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
            "font_size = 30\n\
             flash = no\n\
             bell_attention = \"false\"\n\
             restore_scrollback\n\
             = true\n\
             # check_updates = false\n\
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
    fn saving_a_theme_keeps_the_rest_of_the_file() {
        assert_eq!(with_theme("", "A"), "theme = \"A\"\n");
        assert_eq!(
            with_theme("flash = false\r\n# theme = old\n", "A"),
            "flash = false\n# theme = old\ntheme = \"A\"\n"
        );
        let saved = with_theme("\u{feff}theme = old\nflash = false", "B C");
        assert_eq!(saved, "\u{feff}theme = \"B C\"\nflash = false\n");
        let saved = with_theme("theme = a\ntheme = b # mine\n", "C");
        assert_eq!(Config::parse(&saved).theme, "C", "the last line counts");
    }

    #[test]
    fn config_missing_file_gives_defaults() {
        let path = std::env::temp_dir().join("blitz-no-such-config.toml");
        assert_eq!(Config::read(&path), Config::default());
    }
}
