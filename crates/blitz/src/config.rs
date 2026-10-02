//! Settings: built-in defaults, with a few switches read from
//! `%APPDATA%\blitz\config.toml`.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    /// Follow the Windows app theme.
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub font_family: String,
    /// Used when `font_family` is not installed.
    pub font_fallback: String,
    /// Points; pixels = pt * dpi / 72.
    pub font_size: f32,
    pub line_height: f32,
    pub theme: ThemeMode,
    /// `0xRRGGBB`; `None` uses the theme's accent.
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
            theme: ThemeMode::System,
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
    /// `key = true` or `key = false` per line, `#` starts a comment. Lines
    /// it doesn't understand are skipped, so a typo never stops blitz from
    /// starting.
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        // Notepad may save with a byte order mark.
        for line in text.trim_start_matches('\u{feff}').lines() {
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
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map_or_else(Config::default, |d| {
                Config::read(&d.join("blitz").join("config.toml"))
            })
    }

    /// A missing or unreadable file gives the defaults.
    fn read(path: &Path) -> Config {
        Config::parse(&std::fs::read_to_string(path).unwrap_or_default())
    }
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
    fn config_missing_file_gives_defaults() {
        let path = std::env::temp_dir().join("blitz-no-such-config.toml");
        assert_eq!(Config::read(&path), Config::default());
    }
}
