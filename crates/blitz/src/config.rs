//! Settings. Built-in defaults only for now.

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
            keys: vec![("ctrl+shift+r".into(), "split_right".into())],
        }
    }
}
