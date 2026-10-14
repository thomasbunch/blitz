//! Settings: built-in defaults, with the ones in [`SETTINGS`] and key
//! bindings read from `%APPDATA%\blitz\config.toml`.

use std::path::{Path, PathBuf};

use crate::keymap;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub font_family: String,
    /// Points; pixels = pt * dpi / 72.
    pub font_size: f32,
    /// Space between the panes' lines, as a multiple of the font's own.
    pub line_height: f32,
    /// A theme name, or `light:NAME,dark:NAME` to follow the Windows app
    /// theme; see [`crate::theme::choose`].
    pub theme: String,
    /// A program and its arguments; see [`crate::shell::launch`]. Empty
    /// means detect: pwsh, then Windows PowerShell, then cmd.
    pub shell: String,
    pub shell_integration: bool,
    pub scrollback_lines: usize,
    /// Flash the taskbar button when a session needs attention.
    pub flash: bool,
    /// Which sessions get a Windows notification while blitz is in the
    /// background; one of [`TOASTS`].
    pub toasts: String,
    /// Make a sound when a session needs attention.
    pub sound: bool,
    /// Ctrl+Alt+J brings the user to the session that needs them, from
    /// any program.
    pub global_jump: bool,
    /// Whether BEL, or a notification without the pane's token, in an
    /// unfocused pane asks for attention.
    pub bell_attention: bool,
    /// Look for a newer release on GitHub at start and every six hours.
    pub check_updates: bool,
    /// Reopen the last window's tabs, splits and folders at start.
    pub restore_session: bool,
    /// Reopen the Claude Code session a restored pane was running.
    pub restore_claude: bool,
    /// Save each pane's recent output and show it again on the next start.
    /// Off by default: old output can hold secrets.
    pub restore_scrollback: bool,
    /// Keep the PC from going to sleep while a session works.
    pub keep_awake: bool,
    /// Pixel scenery behind the panes, one of
    /// [`SCENES`](crate::arcade::scenery::SCENES).
    pub scenery: String,
    /// The spark at the foot of the sidebar.
    pub mascot: bool,
    /// A right click copies the selection, or pastes without one.
    pub right_click_paste: bool,
    /// Ctrl+click opens a file at its line through this URI, with
    /// `{path}`, `{line}` and `{col}` filled in, as in
    /// `vscode://file/{path}:{line}:{col}`. Empty opens files with their
    /// program.
    pub editor_uri: String,
    /// Key bindings from `keybind` lines, one per chord, which take the
    /// place of the default for that chord; see [`keymap::binding`].
    pub keys: Vec<keymap::Binding>,
    /// What `text:` bindings type, by [`keymap::Action::SendText`] index.
    pub texts: Vec<Vec<u8>>,
    /// Variables from `env = NAME=VALUE` lines, one per name, for every
    /// new pane's environment.
    pub env: Vec<(String, String)>,
    /// The lines of `config.toml` that set nothing, as (line number,
    /// text): an unknown key, a value that does not fit, or no
    /// `key = value` at all.
    pub ignored: Vec<(usize, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_family: "Cascadia Mono".into(),
            font_size: 11.0,
            line_height: 1.0,
            theme: crate::theme::DEFAULT.into(),
            shell: String::new(),
            shell_integration: true,
            scrollback_lines: 10_000,
            flash: true,
            toasts: "needs-you".into(),
            sound: false,
            global_jump: false,
            bell_attention: true,
            check_updates: true,
            restore_session: true,
            restore_claude: true,
            restore_scrollback: false,
            keep_awake: false,
            scenery: "off".into(),
            mascot: false,
            right_click_paste: true,
            editor_uri: String::new(),
            keys: Vec::new(),
            texts: Vec::new(),
            env: Vec::new(),
            ignored: Vec::new(),
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

/// The values of `toasts`: none, sessions that need the user or failed,
/// and finished ones too.
pub const TOASTS: &[&str] = &["off", "needs-you", "all"];

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
        key: "line_height",
        group: "Appearance",
        label: "Line height",
        help: "Space between the lines in the panes, from 0.8 to 2 times the \
               font's own. The sidebar keeps its spacing.",
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
        key: "keep_awake",
        group: "Sessions",
        label: "Keep the PC awake",
        help: "Keep the PC from going to sleep by itself while a session is \
               working. The screen can still turn off.",
        kind: Kind::Toggle,
        applies: NOW,
    },
    Setting {
        key: "shell",
        group: "Shell",
        label: "Shell",
        help: "The program each new pane runs, with any arguments, as in \
               wsl.exe -d Ubuntu. Automatic picks PowerShell 7, then Windows \
               PowerShell, then cmd.",
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
        key: "right_click_paste",
        group: "Mouse",
        label: "Right-click copy and paste",
        help: "A right click copies the selection, or pastes when nothing is \
               selected. Hold Shift when a program takes the mouse.",
        kind: Kind::Toggle,
        applies: NOW,
    },
    Setting {
        key: "editor_uri",
        group: "Mouse",
        label: "Editor",
        help: "Ctrl+click on a file path opens it at its line in this editor. \
               Without one, a file opens with its program, or shows in \
               Explorer when it has none.",
        kind: Kind::Choice,
        applies: NOW,
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
        key: "toasts",
        group: "Notifications",
        label: "Windows notifications",
        help: "Show a notification when a session needs you or fails while \
               blitz is in the background; All adds finished ones. Clicking it \
               takes you to the session.",
        kind: Kind::Choice,
        applies: NOW,
    },
    Setting {
        key: "sound",
        group: "Notifications",
        label: "Sound",
        help: "Play a sound when a session needs you while blitz is in the \
               background: the notification's, or the Windows default beep \
               without one.",
        kind: Kind::Toggle,
        applies: NOW,
    },
    Setting {
        key: "global_jump",
        group: "Notifications",
        label: "Jump from anywhere",
        help: "Ctrl+Alt+J brings blitz to the front from any program, on the \
               session that needs you. The main window takes it; others leave \
               it alone.",
        kind: Kind::Toggle,
        applies: NOW,
    },
    Setting {
        key: "bell_attention",
        group: "Notifications",
        label: "Bell needs you",
        help: "A bell or a notification in a pane you are not looking at marks \
               its session as needing you until you look. A pane Claude Code's \
               hooks report for leaves it to them.",
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
        help: "Pixel stars, hills or snow drifting behind the panes, faint \
               enough to read over. Still when Windows animations are off.",
        kind: Kind::Choice,
        applies: NOW,
    },
    Setting {
        key: "mascot",
        group: "Easter eggs",
        label: "Spark",
        help: "A little critter at the foot of the sidebar that naps, runs \
               and waves along with your sessions.",
        kind: Kind::Toggle,
        applies: NOW,
    },
    Setting {
        key: "game",
        group: "Easter eggs",
        label: "blitz run",
        help: "A one-button runner for while agents work. Enter starts it, \
               Space jumps, Esc quits; it closes when a session needs you.",
        kind: Kind::Game,
        applies: NOW,
    },
];

impl Config {
    /// The defaults with the settings from a `config.toml` applied: one
    /// `key = value` per line, `#` after a space starts a comment. Lines it
    /// doesn't understand are skipped, so a typo never stops blitz from
    /// starting, and listed in `ignored`.
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        // Notepad may save with a byte order mark.
        let lines = split_lines(text.trim_start_matches('\u{feff}')).0;
        for (i, line) in lines.into_iter().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if !entry(line).is_some_and(|(key, value, _)| c.set(key, value)) {
                c.ignored.push((i + 1, line.into()));
            }
        }
        c
    }

    /// One line that tells the user which lines of `config.toml` were
    /// skipped; `None` when none were. It names the first one's key, not
    /// its value, which may be a token the screen must not show. Nor is
    /// what only looks like a key shown, such as a padded base64 secret
    /// on a line of its own: keys are short, lowercase and snake_case, or
    /// kebab-case as other terminals spell them.
    pub fn ignored_notice(&self) -> Option<String> {
        let ((n, line), more) = self.ignored.split_first()?;
        let key = (line.split_once('=').map(|(k, _)| k.trim())).filter(|k| {
            let word = |b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-');
            k.len() <= 32 && k.bytes().all(word)
        });
        let key = key.map_or_else(String::new, |k| format!(" ({k})"));
        Some(match more.len() {
            0 => format!("config.toml line {n}{key} was skipped"),
            k => format!("config.toml line {n}{key} and {k} more were skipped"),
        })
    }

    /// Whether `other` skips lines this one does not, or the other way
    /// round. Only their text counts: a save from the settings panel can
    /// move a skipped line without changing it.
    pub fn skips_other_lines(&self, other: &Config) -> bool {
        let text = |c: &Config| c.ignored.iter().map(|l| l.1.clone()).collect::<Vec<_>>();
        text(self) != text(other)
    }

    /// Setting `key` as `config.toml` writes it: `true`, `11` or
    /// `"Consolas"`. Empty for an unknown key.
    pub fn get(&self, key: &str) -> String {
        let flag = |b: bool| b.to_string();
        match key {
            "theme" => quote(&self.theme),
            "font_family" => quote(&self.font_family),
            "font_size" => self.font_size.to_string(),
            "line_height" => self.line_height.to_string(),
            "shell" => quote(&self.shell),
            "shell_integration" => flag(self.shell_integration),
            "scrollback_lines" => self.scrollback_lines.to_string(),
            "restore_session" => flag(self.restore_session),
            "restore_claude" => flag(self.restore_claude),
            "restore_scrollback" => flag(self.restore_scrollback),
            "keep_awake" => flag(self.keep_awake),
            "flash" => flag(self.flash),
            "toasts" => quote(&self.toasts),
            "sound" => flag(self.sound),
            "global_jump" => flag(self.global_jump),
            "bell_attention" => flag(self.bell_attention),
            "check_updates" => flag(self.check_updates),
            "scenery" => quote(&self.scenery),
            "mascot" => flag(self.mascot),
            "right_click_paste" => flag(self.right_click_paste),
            "editor_uri" => quote(&self.editor_uri),
            _ => String::new(),
        }
    }

    /// Sets `key` from a value as `config.toml` holds it. Returns false,
    /// changing nothing, for an unknown key or a value of the wrong type or
    /// out of range. On/off and numbers must not be quoted; names may be.
    /// Each `keybind` adds a binding, replacing only one for the same chord,
    /// and each `env` a variable, replacing only one of the same name.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        let text = unquote(value);
        let bare = text.is_none().then_some(value);
        let num = bare.and_then(number);
        let text = text.unwrap_or_else(|| value.to_string());
        match key {
            "keybind" => {
                let b = match keymap::text_binding(&text) {
                    Some((mods, vk, t)) => {
                        let Ok(i) = u16::try_from(self.texts.len()) else {
                            return false;
                        };
                        self.texts.push(t);
                        (mods, vk, Some(keymap::Action::SendText(i)))
                    }
                    None => match keymap::binding(&text) {
                        Some(b) => b,
                        None => return false,
                    },
                };
                self.keys.retain(|k| (k.0, k.1) != (b.0, b.1));
                self.keys.push(b);
            }
            "env" => match text.split_once('=') {
                Some((k, v)) if !k.trim().is_empty() && !text.contains(char::is_control) => {
                    let k = k.trim();
                    self.env.retain(|e| !e.0.eq_ignore_ascii_case(k));
                    // `FOO = bar`, spaced as the line's own `=`.
                    self.env.push((k.into(), v.trim_start().into()));
                }
                _ => return false,
            },
            "theme" | "font_family" if text.is_empty() => return false,
            "theme" => self.theme = text,
            "font_family" => self.font_family = text,
            "shell" => self.shell = text,
            "editor_uri" => self.editor_uri = text,
            "scenery" => match text.to_lowercase() {
                s if crate::arcade::scenery::SCENES.contains(&s.as_str()) => self.scenery = s,
                _ => return false,
            },
            "toasts" => match text.to_lowercase() {
                s if TOASTS.contains(&s.as_str()) => self.toasts = s,
                _ => return false,
            },
            "font_size" => match num.filter(|n| (4.0..=72.0).contains(n)) {
                Some(n) => self.font_size = n as f32,
                None => return false,
            },
            "line_height" => match num.filter(|n| (0.8..=2.0).contains(n)) {
                Some(n) => self.line_height = n as f32,
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
            "keep_awake" => &mut self.keep_awake,
            "flash" => &mut self.flash,
            "sound" => &mut self.sound,
            "global_jump" => &mut self.global_jump,
            "bell_attention" => &mut self.bell_attention,
            "check_updates" => &mut self.check_updates,
            "mascot" => &mut self.mascot,
            "right_click_paste" => &mut self.right_click_paste,
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
        match dir() {
            Some(d) => Config::reload_from(&d.join(FILE)),
            None => Some(Config::default()),
        }
    }

    fn reload_from(path: &Path) -> Option<Config> {
        match std::fs::read(path) {
            Ok(bytes) => Some(Config::parse(&decode(&bytes))),
            // A folder in its place never becomes readable.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound || path.is_dir() => {
                Some(Config::default())
            }
            Err(_) => None,
        }
    }

    /// A missing or unreadable file gives the defaults.
    fn read(path: &Path) -> Config {
        Config::parse(&std::fs::read(path).map_or_else(|_| String::new(), |b| decode(&b)))
    }
}

/// A config file's text. Notepad can save UTF-16 ("Unicode"), and a file
/// that is not UTF-8 is read as ANSI, Windows-1252 as Western Windows
/// writes it, so saving it back as UTF-8 keeps every character.
// ponytail: Windows-1252 only; the system's code page (CP_ACP) for files
// saved on, say, a Japanese or Polish Windows.
pub(crate) fn decode(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = rest.as_chunks().0.iter().map(|&c| unit(c)).collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xff, 0xfe, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => text.to_string(),
            // Line by line: one line an editor saved in the ANSI code page
            // must not turn the UTF-8 around it into mojibake.
            Err(_) => (bytes.split_inclusive(|&b| b == b'\n'))
                .map(|line| match std::str::from_utf8(line) {
                    Ok(line) => line.to_string(),
                    Err(_) => line.iter().map(|&b| windows_1252(b)).collect(),
                })
                .collect(),
        },
    }
}

/// A Windows-1252 byte. 0x80-0x9F are its own; the five it leaves
/// undefined read as their C1 controls, as Windows reads them.
fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9f => HIGH[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}

/// The lines of `text` and the line ending it uses: `\r\n`, `\n`, or a
/// lone `\r` as old Mac editors wrote.
fn split_lines(text: &str) -> (Vec<&str>, &'static str) {
    if text.contains('\n') || !text.contains('\r') {
        let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
        return (text.lines().collect(), nl);
    }
    let body = text.strip_suffix('\r').unwrap_or(text);
    (body.split('\r').collect(), "\r")
}

/// A number as TOML writes it: `11`, `13.5`, `+12`, `100_000`. An
/// underscore must sit between two digits.
fn number(v: &str) -> Option<f64> {
    let b = v.as_bytes();
    let digit = |i: Option<usize>| i.and_then(|i| b.get(i)).is_some_and(u8::is_ascii_digit);
    let ok = (0..b.len())
        .filter(|&i| b[i] == b'_')
        .all(|i| digit(i.checked_sub(1)) && digit(Some(i + 1)));
    ok.then(|| v.replace('_', "").parse().ok()).flatten()
}

/// The key and value of a `key = value` line, and what follows the value,
/// such as a comment. A quoted value keeps its quotes and may hold a `#`;
/// so may another inside a word, as in `env = COLOR=#ff0000`.
fn entry(line: &str) -> Option<(&str, &str, &str)> {
    let (key, rest) = line.split_once('=')?;
    let rest = rest.trim_start();
    let comment = |(i, _): &(usize, &str)| *i == 0 || rest[..*i].ends_with([' ', '\t']);
    let (value, after) = match rest.chars().next() {
        Some(q @ ('"' | '\'')) => rest.split_at(closing(rest, q)? + 1),
        _ => match rest.match_indices('#').find(comment) {
            Some((i, _)) => (rest[..i].trim(), &rest[i..]),
            None => (rest.trim(), ""),
        },
    };
    Some((key.trim(), value, after))
}

/// Where the string that starts `s` with quote `q` ends. A `"` string ends
/// at its first `"` when only a comment or nothing follows it, so a
/// hand-written path ending in `\` reads as it always did, whatever its
/// comment holds. Otherwise a `\"` in it is a quote, and it ends at the
/// next `"` that only a comment or nothing follows, else the first.
fn closing(s: &str, q: char) -> Option<usize> {
    let first = s[1..].find(q)? + 1;
    if q == '\'' {
        return Some(first);
    }
    let ends = |i: usize| {
        let after = s.get(i + 1..).unwrap_or("").trim_start();
        after.is_empty() || after.starts_with('#')
    };
    if ends(first) {
        return Some(first);
    }
    let b = s.as_bytes();
    let end = (first + 1..b.len()).find(|&i| b[i] == b'"' && b[i - 1] != b'\\' && ends(i));
    Some(end.unwrap_or(first))
}

/// The text of a quoted value; `None` when it is not quoted. In a `"`
/// string `\"` is a quote, and so are the unicode escapes [`quote`] writes
/// for a quote and a backslash; any other backslash stays, as Windows
/// paths, UNC ones too, are written by hand.
fn unquote(value: &str) -> Option<String> {
    let q = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let inner = value.strip_prefix(q)?.strip_suffix(q)?;
    if q == '\'' {
        return Some(inner.to_string());
    }
    let mut out = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let unit = |e: &str| tail.get(..6).is_some_and(|t| t.eq_ignore_ascii_case(e));
        let (c, n) = if tail.starts_with("\\\"") {
            ('"', 2)
        } else if unit("\\u0022") {
            ('"', 6)
        } else if unit("\\u005c") {
            ('\\', 6)
        } else {
            ('\\', 1)
        };
        out.push(c);
        rest = &tail[n..];
    }
    out.push_str(rest);
    Some(out)
}

/// `s` as a TOML string. One holding `"` or `\` is a literal string, as a
/// basic one would read those as escapes. One that also holds `'` is a
/// basic string with its `"` and `\` written as unicode escapes, so it
/// holds no `"` before its end and no backslash a path could have.
pub fn quote(s: &str) -> String {
    if !s.contains(['"', '\\']) {
        format!("\"{s}\"")
    } else if !s.contains('\'') {
        format!("'{s}'")
    } else {
        let s = s.replace('\\', "\\u005C").replace('"', "\\u0022");
        format!("\"{s}\"")
    }
}

const FILE: &str = "config.toml";
/// How old another process's temporary file is before it is taken for one
/// a crash left.
const STALE: std::time::Duration = std::time::Duration::from_secs(60);

/// Font families tried in order when `font_family` is not installed;
/// Consolas ships with every Windows.
pub const FALLBACK_FONTS: &[&str] = &["Cascadia Mono", "Consolas", "Courier New"];

/// `%APPDATA%\blitz`, which holds `config.toml` and the themes folder.
pub fn dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("blitz"))
}

/// `config.toml`, for the user to open in an editor: an empty one is
/// made first when there is none, so the editor does not ask.
pub fn file() -> std::io::Result<PathBuf> {
    file_in(&dir().ok_or_else(|| std::io::Error::other("no APPDATA"))?)
}

fn file_in(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE);
    // Appending never changes one that is there.
    (std::fs::OpenOptions::new().append(true).create(true)).open(&path)?;
    Ok(path)
}

/// Sets `key = value` in `config.toml`, keeping the rest of the file; with
/// no value, removes `key` so its default applies. Writes a new file and
/// moves it over the old one, so a failed save never leaves half a file.
pub fn save(key: &str, value: Option<&str>) -> std::io::Result<()> {
    let dir = dir().ok_or_else(|| std::io::Error::other("no APPDATA"))?;
    save_in(&dir, key, value)
}

fn save_in(dir: &Path, key: &str, value: Option<&str>) -> std::io::Result<()> {
    // A control character, such as a newline in a font's name, would end
    // the line and start a setting of its own.
    if value.is_some_and(|v| v.chars().any(char::is_control)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a setting cannot hold a control character",
        ));
    }
    std::fs::create_dir_all(dir)?;
    let mut path = dir.join(FILE);
    // A config.toml linked from elsewhere, as from a dotfiles folder, is
    // written where the link points, so the link stays; one made before
    // the file it points to makes that file.
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        path = match std::fs::canonicalize(&path) {
            Ok(p) => p,
            Err(_) => dir.join(std::fs::read_link(&path)?),
        };
    }
    // Where it went wrong, which is not config.toml when it is a link.
    let named =
        |e: std::io::Error| std::io::Error::new(e.kind(), format!("{}: {e}", path.display()));
    let old = match std::fs::read(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        r => decode(&r.map_err(named)?),
    };
    // Named for this process, so two blitz windows saving at once never
    // write or move each other's file.
    let tmp = path.with_file_name(format!("{FILE}.{}.new", std::process::id()));
    // A crash between the write and the move leaves one that no later
    // process, with another id, replaces. A minute old, it is no save in
    // progress.
    let folder = path.parent().unwrap_or(dir);
    for e in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let name = e.file_name();
        let id = (name.to_str()).and_then(|n| {
            n.strip_prefix(FILE)?
                .strip_prefix('.')?
                .strip_suffix(".new")
        });
        let stale = (e.metadata().and_then(|m| m.modified()).ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if stale && id.is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let saved = std::fs::write(&tmp, with_value(&old, key, value))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if saved.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    saved.map_err(named)
}

/// `text` with its last `key` line, the one that counts, set to `value`,
/// keeping a comment after the old value, or one added before the first
/// `[table]` (or at the end). With no value, every `key` line goes. The
/// file keeps its line endings.
fn with_value(text: &str, key: &str, value: Option<&str>) -> String {
    let bom = if text.starts_with('\u{feff}') {
        "\u{feff}"
    } else {
        ""
    };
    let (lines, nl) = split_lines(text.trim_start_matches('\u{feff}'));
    let mut lines: Vec<String> = lines.into_iter().map(str::to_string).collect();
    let is_key = |l: &String| entry(l).is_some_and(|(k, _, _)| k == key);
    match value {
        Some(v) => match lines.iter().rposition(is_key) {
            Some(i) => {
                let after = entry(&lines[i]).map_or("", |e| e.2).trim_start();
                let comment = if after.starts_with('#') {
                    format!(" {after}")
                } else {
                    String::new()
                };
                lines[i] = format!("{key} = {v}{comment}");
            }
            None => {
                let table = lines.iter().position(|l| l.trim_start().starts_with('['));
                lines.insert(table.unwrap_or(lines.len()), format!("{key} = {v}"));
            }
        },
        None => lines.retain(|l| !is_key(l)),
    }
    if lines.is_empty() {
        return bom.to_string();
    }
    format!("{bom}{}{nl}", lines.join(nl))
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
        // Each line but the comment, by number.
        let lines: Vec<usize> = c.ignored.iter().map(|l| l.0).collect();
        assert_eq!(lines, [1, 2, 3, 4, 5, 6, 7, 9, 10]);
        let notice = "config.toml line 1 (font_size) and 8 more were skipped";
        assert_eq!(c.ignored_notice().as_deref(), Some(notice));
        assert_eq!(
            Config {
                ignored: Vec::new(),
                ..c
            },
            Config::default()
        );
        assert_eq!(Config::parse(""), Config::default());
        let one = Config::parse("\u{feff}\n  # x = 1\nflash = false # ok\r\n  flash = maybe  \r\n");
        let notice = "config.toml line 4 (flash) was skipped";
        assert_eq!(one.ignored_notice().as_deref(), Some(notice));
        // A mistyped key's value stays off the screen.
        let typo = Config::parse("evn = GITHUB_TOKEN=ghp_x\nghp_y\n");
        let notice = "config.toml line 1 (evn) and 1 more were skipped";
        assert_eq!(typo.ignored_notice().as_deref(), Some(notice));
        let other = Config::parse("font-size = 14\n");
        let notice = "config.toml line 1 (font-size) was skipped";
        assert_eq!(other.ignored_notice().as_deref(), Some(notice));
        let secret = Config::parse(
            "env = API_KEY=
dGhpcyBpcyBhIHNlY3JldA==
",
        );
        let notice = "config.toml line 2 was skipped";
        assert_eq!(secret.ignored_notice().as_deref(), Some(notice));
        assert_eq!(Config::parse("flash = false # ok\n").ignored_notice(), None);
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
    fn config_reads_key_bindings() {
        let c = Config::parse(
            "keybind = ctrl+shift+r=new_tab\n\
             keybind = \"ctrl+shift+w=none\" # mine\n\
             keybind = ctrl+shift+r=split_down\n\
             keybind = ctrl+shift+q=quit\n\
             keybind = ctrl+shift\n",
        );
        let want: Vec<keymap::Binding> = ["ctrl+shift+w=none", "ctrl+shift+r=split_down"]
            .into_iter()
            .filter_map(keymap::binding)
            .collect();
        assert_eq!(c.keys, want, "the last line for a chord counts");
        assert_eq!(c.font_size, Config::default().font_size);
    }

    #[test]
    fn config_reads_text_bindings() {
        let c = Config::parse(
            r#"keybind = ctrl+shift+e=text:claude\r # start it
keybind = "ctrl+shift+y=text:a=b # not a comment\e"
keybind = ctrl+shift+e=text:git status\r
keybind = ctrl+shift+n=text:
"#,
        );
        let send = |i| Some(keymap::Action::SendText(i));
        assert_eq!(c.keys, [(3, 0x59, send(1)), (3, 0x45, send(2))]);
        assert_eq!(c.texts[0], b"claude\r", "replaced, but kept");
        assert_eq!(c.texts[1], b"a=b # not a comment\x1b");
        assert_eq!(c.texts[2], b"git status\r");
    }

    #[test]
    fn a_skipped_line_moved_by_a_save_is_no_news() {
        let old = Config::parse(
            "flash = false
[colors]
",
        );
        // The panel adds a setting before the first table.
        let saved = Config::parse(&with_value(
            "flash = false
[colors]
",
            "font_size",
            Some("12"),
        ));
        assert_ne!(saved.ignored, old.ignored, "the line moved");
        assert!(!saved.skips_other_lines(&old));
        assert!(
            Config::parse(
                "[x]
"
            )
            .skips_other_lines(&old)
        );
        assert!(Config::default().skips_other_lines(&old));
    }

    #[test]
    fn config_reads_variables_for_panes() {
        let c = Config::parse(
            "env = RUST_LOG=debug\n\
             env = \"EDITOR=code --wait\" # mine\n\
             env = rust_log=info\n\
             env = EMPTY=\n\
             env = =C:=x\n\
             env = NOEQUALS\n\
             env = COLOR=#ff0000 # red\n\
             env = SPACED = out\n",
        );
        // The last line for a name counts, whatever its case.
        let want = [
            ("EDITOR", "code --wait"),
            ("rust_log", "info"),
            ("EMPTY", ""),
            ("COLOR", "#ff0000"),
            ("SPACED", "out"),
        ];
        assert_eq!(c.env, want.map(|(k, v)| (k.to_string(), v.to_string())));
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
    fn help_text_has_single_spaces() {
        // The settings search matches the help as written.
        for s in SETTINGS {
            assert!(!s.help.contains("  "), "{}: {:?}", s.key, s.help);
        }
    }

    #[test]
    fn saving_a_value_keeps_the_rest_of_the_file() {
        let theme = |text: &str, name: &str| with_value(text, "theme", Some(&quote(name)));
        assert_eq!(theme("", "A"), "theme = \"A\"\n");
        // Windows line endings stay, and win over a stray Unix one.
        assert_eq!(
            theme("flash = false\r\n# theme = old\n", "A"),
            "flash = false\r\n# theme = old\r\ntheme = \"A\"\r\n"
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

    /// A folder for one test, gone when the test ends, even if it fails.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir =
                std::env::temp_dir().join(format!("blitz-config-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Temp(dir)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
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
    fn config_reads_which_sessions_get_notifications() {
        assert_eq!(Config::default().toasts, "needs-you");
        assert_eq!(Config::parse("toasts = \"All\"").toasts, "all");
        assert_eq!(Config::parse("toasts = off").toasts, "off");
        let c = Config::parse("toasts = \"all\"\ntoasts = \"done\"\n");
        assert_eq!(c.toasts, "all", "an unknown value is skipped");
    }

    #[test]
    fn config_missing_file_gives_defaults() {
        let t = Temp::new("missing");
        let path = t.0.join(FILE);
        assert_eq!(Config::read(&path), Config::default());
        assert_eq!(Config::reload_from(&path), Some(Config::default()));
    }

    #[test]
    fn reload_reads_the_file_and_waits_out_what_it_cannot_read() {
        let t = Temp::new("reload");
        let path = t.0.join(FILE);
        std::fs::write(&path, "flash = false\n").unwrap();
        assert!(!Config::reload_from(&path).expect("readable").flash);
        // A folder where the file should be is no file: the defaults.
        let folder = t.0.join("folder");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(Config::reload_from(&folder), Some(Config::default()));
        assert_eq!(Config::read(&folder), Config::default());
    }

    /// The panel's rows that are settings in the file, not blitz run.
    fn values() -> impl Iterator<Item = &'static Setting> {
        SETTINGS.iter().filter(|s| s.kind != Kind::Game)
    }

    #[test]
    fn every_setting_round_trips_a_value_other_than_its_default() {
        let mut c = Config::default();
        for (k, v) in [
            ("theme", "\"Rose Pine\""),
            ("font_family", "\"Consolas\""),
            ("font_size", "13.5"),
            ("line_height", "1.2"),
            ("shell", r"'C:\x\sh.exe'"),
            ("shell_integration", "false"),
            ("scrollback_lines", "0"),
            ("restore_session", "false"),
            ("restore_claude", "false"),
            ("restore_scrollback", "true"),
            ("keep_awake", "true"),
            ("flash", "false"),
            ("toasts", "\"all\""),
            ("sound", "true"),
            ("global_jump", "true"),
            ("bell_attention", "false"),
            ("check_updates", "false"),
            ("scenery", "\"snow\""),
            ("mascot", "true"),
            ("right_click_paste", "false"),
            ("editor_uri", "\"cursor://file/{path}:{line}:{col}\""),
        ] {
            assert!(c.set(k, v), "{k} = {v}");
        }
        let d = Config::default();
        for s in values() {
            assert_ne!(c.get(s.key), d.get(s.key), "{} kept its default", s.key);
        }
        let text: String = values()
            .map(|s| format!("{} = {}\n", s.key, c.get(s.key)))
            .collect();
        assert_eq!(Config::parse(&text), c);
        // As the settings panel writes them, one at a time.
        let mut file = String::new();
        for s in values() {
            file = with_value(&file, s.key, Some(&c.get(s.key)));
        }
        assert_eq!(Config::parse(&file), c);
    }

    #[test]
    fn readme_documents_every_setting() {
        let readme = include_str!("../../../README.md");
        let d = Config::default();
        for s in values() {
            let row = format!("| `{}` | `{}` |", s.key, d.get(s.key));
            assert!(readme.contains(&row), "README.md has no row {row}");
        }
        for line in readme.lines().filter(|l| l.starts_with("| `")) {
            let key = line[3..].split('`').next().unwrap_or_default();
            // Key bindings and variables are read too, though the panel does
            // not show them.
            let known = ["keybind", "env"].contains(&key) || SETTINGS.iter().any(|s| s.key == key);
            assert!(
                known,
                "README.md documents {key}, which blitz does not read"
            );
        }
    }

    #[test]
    fn one_odd_byte_keeps_the_other_settings() {
        // Notepad's ANSI: not UTF-8.
        let ansi = b"# R\xe9glages\nflash = false\nfont_family = \"Caf\xe9\"\n";
        let c = Config::parse(&decode(ansi));
        assert!(!c.flash);
        assert_eq!(c.font_family, "Café");
        // One ANSI line in a UTF-8 file leaves the UTF-8 lines as they are.
        assert_eq!(
            decode(b"theme = \"Ros\xc3\xa9\"\r\n# caf\xe9\n"),
            "theme = \"Rosé\"\r\n# café\n"
        );
        // Windows-1252, the ANSI of Western Windows, with its own 0x80-0x9F
        // and the five bytes it leaves undefined as their C1 controls.
        assert_eq!(decode(b"\x80\x8a\x96\x99\x9f\xa0\xff"), "€Š–™Ÿ\u{a0}ÿ");
        assert_eq!(
            decode(b"\x81\x8d\x8f\x90\x9d"),
            "\u{81}\u{8d}\u{8f}\u{90}\u{9d}"
        );
        // Notepad's "Unicode": UTF-16 with a byte order mark.
        let text = "flash = false\r\ntheme = \"Rose Pine\"\r\n";
        let le: Vec<u8> = [0xff, 0xfe]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let be: Vec<u8> = [0xfe, 0xff]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        for bytes in [le, be] {
            let c = Config::parse(&decode(&bytes));
            assert!(!c.flash);
            assert_eq!(c.theme, "Rose Pine");
        }
        // Through the file, at start and on a reload.
        let t = Temp::new("ansi");
        let path = t.0.join(FILE);
        std::fs::write(&path, ansi).unwrap();
        assert!(!Config::read(&path).flash);
        assert!(!Config::reload_from(&path).expect("readable").flash);
    }

    #[test]
    fn numbers_take_what_toml_writes_and_nothing_out_of_range() {
        let ok = |k: &str, v: &str| Config::default().set(k, v);
        for v in ["4", "72", "+12", "1e1", "13.5", "1_1"] {
            assert!(ok("font_size", v), "font_size = {v}");
        }
        for v in [
            "NaN", "nan", "inf", "-inf", "3.99", "72.01", "-1", "1e308", "", "12pt", "0x10",
            "\"12\"", "_12", "12_", "1__2", "1_.5",
        ] {
            assert!(!ok("font_size", v), "font_size = {v}");
        }
        for v in ["0", "100000", "100_000", "1e5", "-0"] {
            assert!(ok("scrollback_lines", v), "scrollback_lines = {v}");
        }
        for v in ["-1", "100001", "1e400", "0.5", "inf", "NaN"] {
            assert!(!ok("scrollback_lines", v), "scrollback_lines = {v}");
        }
        for v in ["0.8", "1", "1.25", "2"] {
            assert!(ok("line_height", v), "line_height = {v}");
        }
        for v in ["0.79", "2.01", "0", "-1", "NaN", "\"1.2\""] {
            assert!(!ok("line_height", v), "line_height = {v}");
        }
        let c = Config::parse("font_size = 1_2\nscrollback_lines = 20_000");
        assert_eq!((c.font_size, c.scrollback_lines), (12.0, 20_000));
    }

    #[test]
    fn names_must_not_be_empty() {
        let mut c = Config::default();
        assert!(!c.set("font_family", "\"\""));
        assert!(!c.set("theme", "''"));
        assert_eq!(c, Config::default());
        // An empty shell is the automatic one.
        assert!(c.set("shell", "\"\""));
    }

    #[test]
    fn quote_round_trips_any_name() {
        for s in [
            r"C:\a\b",
            "it's",
            r#"a"b"#,
            r#"O'Neil "x""#,
            r"C:\it's\sh.exe",
            r#"'"'"#,
            r#"'"\"#,
            r#"it's "C:\x\""#,
            r#"\\'\""#,
            r#"it's a" # b"#,
            r#"\u0022'""#,
            r"\\server\share\x",
            "plain",
            "",
            "No #1",
        ] {
            let line = format!("font_family = {} # mine", quote(s));
            assert_eq!(
                Config::parse(&line).font_family,
                if s.is_empty() { "Cascadia Mono" } else { s },
                "{line}"
            );
        }
        // Hand-written paths read as before: a backslash that escapes
        // nothing stays, and one at the end does not hold the string open.
        assert_eq!(Config::parse(r#"shell = "C:\tools\""#).shell, r"C:\tools\");
        assert_eq!(
            Config::parse(r#"shell = "C:\x\sh.exe""#).shell,
            r"C:\x\sh.exe"
        );
        // A UNC path keeps its two backslashes.
        assert_eq!(
            Config::parse(r#"shell = "\\server\share\pwsh.exe""#).shell,
            r"\\server\share\pwsh.exe"
        );
        // Nor does a quote in a comment after it, and saving keeps that
        // comment.
        let six = r#"shell = "C:\tools\" # the 6""#;
        assert_eq!(Config::parse(six).shell, r"C:\tools\");
        let fast = r#"shell = "C:\tools\" # the "fast" one"#;
        assert_eq!(Config::parse(fast).shell, r"C:\tools\");
        assert_eq!(
            with_value(fast, "shell", Some("'x'")),
            "shell = 'x' # the \"fast\" one\n"
        );
        // An escaped quote still ends a string when only a comment follows.
        let name = r#"font_family = "a\"b" # "x""#;
        assert_eq!(Config::parse(name).font_family, r#"a"b"#);
    }

    #[test]
    fn a_value_with_a_control_character_is_not_saved() {
        let dir = std::env::temp_dir().join(format!("blitz-control-{}", std::process::id()));
        // A hostile font could name itself so, to add a shell of its own.
        let name = quote("Evil\nshell = 'C:\\x\\evil.exe'");
        let saved = save_in(&dir, "font_family", Some(&name)).map_err(|e| e.kind());
        let wrote = dir.join(FILE).exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            (saved, wrote),
            (Err(std::io::ErrorKind::InvalidInput), false)
        );
    }

    #[test]
    fn parse_ignores_what_toml_would_not_read() {
        // Keys and booleans are lower case.
        let c = Config::parse("Flash = false\nbell_attention = False\n");
        assert_eq!(c.ignored.len(), 2);
        assert_eq!(
            Config {
                ignored: Vec::new(),
                ..c
            },
            Config::default()
        );
        // A later line that does not read keeps the earlier one.
        assert!(!Config::parse("flash = false\nflash = maybe\n").flash);
        // A later line that reads wins.
        assert!(Config::parse("flash = false\nflash = true\n").flash);
        // Old Mac line endings are lines too.
        let c = Config::parse("flash = false\rcheck_updates = false\r");
        assert!(!c.flash && !c.check_updates);
    }

    #[test]
    fn saving_keeps_comments_line_endings_and_tables() {
        let flash = |text: &str| with_value(text, "flash", Some("true"));
        assert_eq!(flash("flash = false # mine\n"), "flash = true # mine\n");
        assert_eq!(
            flash("flash = \"x\"   # quoted\n"),
            "flash = true # quoted\n"
        );
        assert_eq!(flash("flash = false junk\n"), "flash = true\n");
        assert_eq!(
            flash("# a\r\nflash = false\r\ntheme = \"A\"\r\n"),
            "# a\r\nflash = true\r\ntheme = \"A\"\r\n"
        );
        assert_eq!(
            flash("theme = \"A\"\rx = 1\r"),
            "theme = \"A\"\rx = 1\rflash = true\r"
        );
        // TOML puts a key after a [table] in the table.
        assert_eq!(
            flash("theme = \"A\"\n[other]\nkey = 1\n"),
            "theme = \"A\"\nflash = true\n[other]\nkey = 1\n"
        );
        assert_eq!(
            with_value("a = 1\r\nflash = false\r\n", "flash", None),
            "a = 1\r\n"
        );
    }

    #[test]
    fn save_replaces_the_file_whole() {
        let t = Temp::new("save");
        let dir = t.0.join("new");
        save_in(&dir, "flash", Some("false")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(FILE)).unwrap(),
            "flash = false\n"
        );
        save_in(&dir, "theme", Some("\"A\"")).unwrap();
        save_in(&dir, "flash", None).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(FILE)).unwrap(),
            "theme = \"A\"\n"
        );
        // Only config.toml is left: no temporary file.
        let names: Vec<_> = (std::fs::read_dir(&dir).unwrap().flatten())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, [FILE]);
        // A file in the old encoding comes back as UTF-8, every character
        // in it kept.
        std::fs::write(dir.join(FILE), b"# R\xe9glages\nflash = false\n").unwrap();
        save_in(&dir, "theme", Some("\"B\"")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(FILE)).unwrap(),
            "# Réglages\nflash = false\ntheme = \"B\"\n"
        );
    }

    #[test]
    fn opening_the_file_makes_one_and_keeps_one_that_is_there() {
        let t = Temp::new("open");
        let dir = t.0.join("new");
        let path = file_in(&dir).unwrap();
        assert_eq!(path, dir.join(FILE));
        assert_eq!(std::fs::read(&path).unwrap(), b"");
        std::fs::write(&path, "flash = false\n").unwrap();
        file_in(&dir).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"flash = false\n");
    }

    #[test]
    fn a_failed_save_leaves_no_temporary_file() {
        let t = Temp::new("failed");
        // A folder where config.toml should be: the move fails.
        std::fs::create_dir(t.0.join(FILE)).unwrap();
        assert!(save_in(&t.0, "flash", Some("false")).is_err());
        let names: Vec<_> = (std::fs::read_dir(&t.0).unwrap().flatten())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, [FILE]);
    }

    #[test]
    fn save_writes_through_a_linked_file() {
        #[cfg(unix)]
        use std::os::unix::fs::symlink as link;
        #[cfg(windows)]
        use std::os::windows::fs::symlink_file as link;
        let t = Temp::new("link");
        let real = t.0.join("dotfiles");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("blitz.toml"), "# mine\n").unwrap();
        let dir = t.0.join("blitz");
        std::fs::create_dir(&dir).unwrap();
        if let Err(e) = link(real.join("blitz.toml"), dir.join(FILE)) {
            // Windows lets only admins and developer mode make links;
            // CI's runners are admins.
            eprintln!("skipped, cannot make a link here: {e}");
            return;
        }
        save_in(&dir, "flash", Some("false")).unwrap();
        let meta = std::fs::symlink_metadata(dir.join(FILE)).unwrap();
        assert!(meta.file_type().is_symlink(), "the link stays");
        let target = std::fs::read_to_string(real.join("blitz.toml")).unwrap();
        assert_eq!(target, "# mine\nflash = false\n");
        let left: Vec<_> = (std::fs::read_dir(&real).unwrap().flatten())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["blitz.toml"]);
    }

    #[test]
    fn save_clears_what_a_crash_left() {
        let t = Temp::new("stale");
        let tmp = |id: &str| t.0.join(format!("{FILE}.{id}.new"));
        for id in ["4294967295", "4294967294", "x"] {
            std::fs::write(tmp(id), "x").unwrap();
        }
        let an_hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for id in ["4294967295", "x"] {
            let f = std::fs::File::options().write(true).open(tmp(id)).unwrap();
            f.set_modified(an_hour_ago).unwrap();
        }
        save_in(&t.0, "flash", Some("false")).unwrap();
        assert!(!tmp("4294967295").exists(), "a crashed save's");
        assert!(tmp("4294967294").exists(), "another window's, saving now");
        assert!(tmp("x").exists(), "not one of blitz's");
    }

    #[test]
    fn save_makes_the_file_a_link_points_to() {
        #[cfg(unix)]
        use std::os::unix::fs::symlink as link;
        #[cfg(windows)]
        use std::os::windows::fs::symlink_file as link;
        let t = Temp::new("dangling");
        std::fs::create_dir(t.0.join("dotfiles")).unwrap();
        let dir = t.0.join("blitz");
        std::fs::create_dir(&dir).unwrap();
        // Made before its target, and relative to its own folder.
        let to = Path::new("..").join("dotfiles").join("blitz.toml");
        if let Err(e) = link(&to, dir.join(FILE)) {
            eprintln!("skipped, cannot make a link here: {e}");
            return;
        }
        save_in(&dir, "flash", Some("false")).unwrap();
        let meta = std::fs::symlink_metadata(dir.join(FILE)).unwrap();
        assert!(meta.file_type().is_symlink(), "the link stays");
        let target = std::fs::read_to_string(t.0.join("dotfiles").join("blitz.toml")).unwrap();
        assert_eq!(target, "flash = false\n");
        // A link into a folder that is not there says where it points.
        std::fs::remove_file(dir.join(FILE)).unwrap();
        link(t.0.join("gone").join("blitz.toml"), dir.join(FILE)).unwrap();
        let err = save_in(&dir, "flash", Some("true")).unwrap_err();
        assert!(err.to_string().contains("gone"), "{err}");
    }
}
