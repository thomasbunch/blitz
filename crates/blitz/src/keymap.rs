//! Shortcut chords, the default key map and the user's bindings, and
//! translating raw key messages into key input for the terminal.

use vt::{Key, KeyInput, Locks, Mods};

use crate::layout::Dir;

/// What a shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Copy the selection, or one that output just rewrote. Without one
    /// the key goes to the program.
    Copy,
    /// Copy the selection without Claude Code's gutter and the indent its
    /// lines share.
    CopyUnindented,
    /// Paste clipboard text. Without text the key goes to the program.
    Paste,
    /// Select all of the focused pane's text, its scrollback too.
    SelectAll,
    /// Select the last command's output: the lines between blitz's last
    /// two prompts.
    SelectOutput,
    /// Scroll the main screen by a page; positive is up.
    ScrollPage(i8),
    /// Scroll the main screen to the top of the scrollback (1) or to the
    /// bottom (-1).
    ScrollEnd(i8),
    /// Drop the scrollback, and what the console host keeps of it.
    ClearScrollback,
    /// Undo the modes a crashed program left on, such as mouse reports or
    /// a hidden cursor.
    Reset,
    NewTab,
    ClosePane,
    /// Close every pane of the active tab.
    CloseTab,
    /// Open the pane closed last again, beside the focused one, resuming
    /// its Claude Code session.
    ReopenClosed,
    /// Next (1) or previous (-1) tab.
    CycleTab(i8),
    /// Move the tab right (1) or left (-1).
    MoveTab(i8),
    /// Take the focused pane out of its tab into a new one.
    PaneToNewTab,
    /// Tab 1 to 8, 0-based. With no such tab the key does nothing.
    GoToTab(u8),
    /// The last tab, whatever its number.
    LastTab,
    SplitRight,
    SplitDown,
    /// Split right and start Claude Code in the new pane's shell.
    NewClaude,
    /// [`Action::NewClaude`], opening its agent teammates as panes in a
    /// tab beside it.
    NewTeam,
    Focus(Dir),
    /// Move the nearest divider of the focused pane toward `Dir`.
    Resize(Dir),
    /// Trade places with the pane in `Dir`.
    Swap(Dir),
    JumpToAttention,
    /// Open the command palette on the list of sessions, or close it.
    GoToSession,
    ToggleSidebar,
    /// Install the newer release, or without one look for it now.
    Update,
    /// Open the theme picker, or close it unchanged.
    ThemePicker,
    /// Open or close the settings panel.
    Settings,
    /// Open or close the keyboard shortcuts.
    Keys,
    /// Open `config.toml` in an editor.
    OpenConfig,
    /// Open the themes folder in Explorer.
    OpenThemes,
    /// Show only the focused pane, filling the tab, or every pane again.
    Zoom,
    /// Give the tab's panes equal space.
    Equalize,
    /// Make the font a point bigger (1) or smaller (-1) until blitz
    /// restarts, or go back to the size in the settings (0).
    FontSize(i8),
    /// Fill the monitor without a frame, or go back.
    Fullscreen,
    /// Open or close the command palette.
    Palette,
    /// Scroll to the previous (1) or next (-1) prompt. With none that way,
    /// or on the alternate screen, the key goes to the program.
    JumpToPrompt(i8),
    /// Open the find bar on the focused pane, or close it.
    Find,
    /// Show the files in the focused pane's folder over it, or close them.
    Files,
    /// Name the focused session on the command palette's line; an empty
    /// name gives back the one blitz picks.
    RenameSession,
    /// The same for the focused session's tab.
    RenameTab,
    /// Copy the hooks for Claude Code's settings, for a Claude Code that
    /// does not load blitz's own.
    ClaudeSetup,
    /// Open or close the usage panel: what Claude Code's sessions would
    /// cost this week.
    Usage,
    /// Open the window menu, as Alt+Space does in other windows.
    SystemMenu,
    /// Type what a `text:` binding holds, from [`Config::texts`] by index.
    ///
    /// [`Config::texts`]: crate::config::Config::texts
    SendText(u16),
    /// Label the URLs, paths and commit hashes in view, to copy or open
    /// one by its label.
    QuickSelect,
    /// Open a new GitHub issue with what a report needs to know about
    /// this blitz filled in.
    ReportIssue,
}

/// Every action a key can be bound to, with its name in `config.toml` and
/// how the command palette shows it. One line each, in palette order.
#[rustfmt::skip]
pub const ACTIONS: &[(Action, &str, &str)] = &[
    (Action::Copy, "copy", "Copy"),
    (Action::CopyUnindented, "copy_without_indent", "Copy without indent"),
    (Action::Paste, "paste", "Paste"),
    (Action::SelectAll, "select_all", "Select all"),
    (Action::SelectOutput, "select_last_output", "Select the last command's output"),
    (Action::ScrollPage(1), "scroll_page_up", "Scroll up a page"),
    (Action::ScrollPage(-1), "scroll_page_down", "Scroll down a page"),
    (Action::ScrollEnd(1), "scroll_to_top", "Scroll to the top"),
    (Action::ScrollEnd(-1), "scroll_to_bottom", "Scroll to the bottom"),
    (Action::ClearScrollback, "clear_scrollback", "Clear the scrollback and screen"),
    (Action::Reset, "reset_terminal", "Reset the terminal"),
    (Action::NewTab, "new_tab", "New tab"),
    (Action::ClosePane, "close_pane", "Close pane"),
    (Action::CloseTab, "close_tab", "Close tab"),
    (Action::RenameSession, "rename_session", "Rename session"),
    (Action::RenameTab, "rename_tab", "Rename tab"),
    (Action::ReopenClosed, "reopen_closed", "Reopen the last closed pane"),
    (Action::CycleTab(1), "next_tab", "Next tab"),
    (Action::CycleTab(-1), "previous_tab", "Previous tab"),
    (Action::MoveTab(-1), "move_tab_left", "Move the tab left"),
    (Action::MoveTab(1), "move_tab_right", "Move the tab right"),
    (Action::PaneToNewTab, "move_pane_to_new_tab", "Move the pane to a new tab"),
    (Action::SplitRight, "split_right", "Split right"),
    (Action::SplitDown, "split_down", "Split down"),
    (Action::NewClaude, "new_claude", "New Claude Code session"),
    (Action::NewTeam, "new_agent_team", "New agent team"),
    (Action::Focus(Dir::Left), "focus_left", "Focus the pane on the left"),
    (Action::Focus(Dir::Right), "focus_right", "Focus the pane on the right"),
    (Action::Focus(Dir::Up), "focus_up", "Focus the pane above"),
    (Action::Focus(Dir::Down), "focus_down", "Focus the pane below"),
    (Action::JumpToAttention, "jump_to_attention", "Jump to the next session that needs you"),
    (Action::GoToSession, "go_to_session", "Go to a session"),
    (Action::ToggleSidebar, "toggle_sidebar", "Expand or collapse the sidebar"),
    (Action::Update, "update", "Update blitz"),
    (Action::ThemePicker, "theme_picker", "Pick a theme"),
    (Action::Settings, "settings", "Settings"),
    (Action::Keys, "keys", "Keyboard shortcuts"),
    (Action::OpenConfig, "open_config", "Open config.toml"),
    (Action::OpenThemes, "open_themes", "Open themes folder"),
    (Action::Zoom, "zoom", "Zoom the pane, or show every pane"),
    (Action::Resize(Dir::Left), "resize_left", "Move the divider left"),
    (Action::Resize(Dir::Right), "resize_right", "Move the divider right"),
    (Action::Resize(Dir::Up), "resize_up", "Move the divider up"),
    (Action::Resize(Dir::Down), "resize_down", "Move the divider down"),
    (Action::Swap(Dir::Left), "swap_left", "Swap with the pane on the left"),
    (Action::Swap(Dir::Right), "swap_right", "Swap with the pane on the right"),
    (Action::Swap(Dir::Up), "swap_up", "Swap with the pane above"),
    (Action::Swap(Dir::Down), "swap_down", "Swap with the pane below"),
    (Action::Equalize, "equalize", "Give the panes equal space"),
    (Action::FontSize(1), "font_size_up", "Bigger font"),
    (Action::FontSize(-1), "font_size_down", "Smaller font"),
    (Action::FontSize(0), "font_size_reset", "Font size from the settings"),
    (Action::Fullscreen, "fullscreen", "Full screen"),
    (Action::Palette, "command_palette", "Command palette"),
    (Action::JumpToPrompt(1), "previous_prompt", "Scroll to the previous prompt"),
    (Action::JumpToPrompt(-1), "next_prompt", "Scroll to the next prompt"),
    (Action::Find, "find", "Find in the scrollback"),
    (Action::Files, "files", "Browse the files in this folder"),
    (Action::ClaudeSetup, "claude_setup", "Claude Code setup"),
    (Action::Usage, "usage", "Claude Code usage and cost"),
    (Action::SystemMenu, "system_menu", "Window menu"),
    (Action::QuickSelect, "quick_select", "Quick select a link, path or hash"),
    (Action::ReportIssue, "report_issue", "Report an issue"),
    (Action::GoToTab(0), "go_to_tab_1", "Go to tab 1"),
    (Action::GoToTab(1), "go_to_tab_2", "Go to tab 2"),
    (Action::GoToTab(2), "go_to_tab_3", "Go to tab 3"),
    (Action::GoToTab(3), "go_to_tab_4", "Go to tab 4"),
    (Action::GoToTab(4), "go_to_tab_5", "Go to tab 5"),
    (Action::GoToTab(5), "go_to_tab_6", "Go to tab 6"),
    (Action::GoToTab(6), "go_to_tab_7", "Go to tab 7"),
    (Action::GoToTab(7), "go_to_tab_8", "Go to tab 8"),
    (Action::LastTab, "last_tab", "Go to the last tab"),
];

/// A key binding: modifiers, virtual key, and the action, or `None` where
/// the key goes to the program.
pub type Binding = (u8, u16, Option<Action>);

/// A chord: modifiers and virtual key.
pub type Chord = (u8, u16);

/// A change to the key bindings: the `keybind` line for a chord, as
/// config.toml writes it after `keybind = `, or `None` to drop the
/// chord's lines so its default applies.
pub type Rebind = (Chord, Option<String>);

const CTRL: u8 = 1;
const SHIFT: u8 = 2;
const ALT: u8 = 4;
/// Command on a Mac, the Windows key or Super elsewhere.
const CMD: u8 = 8;

/// Default shortcuts as (modifiers, virtual key, action). Keys are matched
/// by virtual-key code: letter codes follow the key labels on Latin
/// layouts and the US positions on others, and digits and arrows are
/// physical keys everywhere.
#[cfg(not(target_os = "macos"))]
const DEFAULT_KEYS: &[(u8, u16, Action)] = &[
    (CTRL, b'C' as u16, Action::Copy),
    (CTRL | SHIFT, b'C' as u16, Action::Copy),
    (CTRL, 0x2d, Action::Copy),
    (CTRL, b'V' as u16, Action::Paste),
    (CTRL | SHIFT, b'V' as u16, Action::Paste),
    (SHIFT, 0x2d, Action::Paste),
    (SHIFT, 0x21, Action::ScrollPage(1)),
    (SHIFT, 0x22, Action::ScrollPage(-1)),
    // Not Shift+Home and Shift+End: PSReadLine selects with them.
    (CTRL | SHIFT, 0x24, Action::ScrollEnd(1)),
    (CTRL | SHIFT, 0x23, Action::ScrollEnd(-1)),
    (CTRL | SHIFT, b'T' as u16, Action::NewTab),
    (CTRL | SHIFT, b'W' as u16, Action::ClosePane),
    (CTRL, 0x09, Action::CycleTab(1)),
    (CTRL | SHIFT, 0x09, Action::CycleTab(-1)),
    (CTRL | SHIFT, 0x21, Action::MoveTab(-1)),
    (CTRL | SHIFT, 0x22, Action::MoveTab(1)),
    (CTRL | SHIFT, b'R' as u16, Action::SplitRight),
    (CTRL | SHIFT, b'D' as u16, Action::SplitDown),
    (CTRL | ALT, 0x25, Action::Focus(Dir::Left)),
    (CTRL | ALT, 0x26, Action::Focus(Dir::Up)),
    (CTRL | ALT, 0x27, Action::Focus(Dir::Right)),
    (CTRL | ALT, 0x28, Action::Focus(Dir::Down)),
    (ALT | SHIFT, 0x25, Action::Resize(Dir::Left)),
    (ALT | SHIFT, 0x26, Action::Resize(Dir::Up)),
    (ALT | SHIFT, 0x27, Action::Resize(Dir::Right)),
    (ALT | SHIFT, 0x28, Action::Resize(Dir::Down)),
    (CTRL | ALT | SHIFT, 0x25, Action::Swap(Dir::Left)),
    (CTRL | ALT | SHIFT, 0x26, Action::Swap(Dir::Up)),
    (CTRL | ALT | SHIFT, 0x27, Action::Swap(Dir::Right)),
    (CTRL | ALT | SHIFT, 0x28, Action::Swap(Dir::Down)),
    (CTRL | SHIFT, b'J' as u16, Action::JumpToAttention),
    (CTRL | SHIFT, b'O' as u16, Action::GoToSession),
    (CTRL | SHIFT, b'B' as u16, Action::ToggleSidebar),
    (CTRL | SHIFT, b'U' as u16, Action::Update),
    (CTRL | SHIFT, b'K' as u16, Action::ThemePicker),
    // VK_OEM_COMMA: the comma key on every layout.
    (CTRL, 0xbc, Action::Settings),
    // VK_OEM_2, the / key: Ctrl+? on a US layout, which would reach the
    // program as ^?, a second Backspace. Not Ctrl+/: that is ^_, Claude
    // Code's undo.
    (CTRL | SHIFT, 0xbf, Action::Keys),
    (CTRL | SHIFT, b'Z' as u16, Action::Zoom),
    // VK_OEM_PLUS and VK_OEM_MINUS, with Shift too where + needs it, and
    // the keypad's. Ctrl+Shift+- still goes to the program: it is Claude
    // Code's undo.
    (CTRL, 0xbb, Action::FontSize(1)),
    (CTRL | SHIFT, 0xbb, Action::FontSize(1)),
    (CTRL, 0xbd, Action::FontSize(-1)),
    (CTRL, b'0' as u16, Action::FontSize(0)),
    (CTRL, 0x6b, Action::FontSize(1)),
    (CTRL, 0x6d, Action::FontSize(-1)),
    (CTRL, 0x60, Action::FontSize(0)),
    // F11.
    (0, 0x7a, Action::Fullscreen),
    (CTRL | SHIFT, b'P' as u16, Action::Palette),
    (CTRL | SHIFT, 0x26, Action::JumpToPrompt(1)),
    (CTRL | SHIFT, 0x28, Action::JumpToPrompt(-1)),
    (CTRL | SHIFT, b'F' as u16, Action::Find),
    (CTRL | SHIFT, b'E' as u16, Action::Files),
    // Only Windows has a window menu; elsewhere Alt+Space is Meta+Space.
    #[cfg(windows)]
    (ALT, 0x20, Action::SystemMenu),
    (CTRL | SHIFT, 0x20, Action::QuickSelect),
];

/// The defaults on a Mac, where shortcuts take Command, as in Terminal,
/// and Ctrl keys all go to the program.
#[cfg(target_os = "macos")]
const DEFAULT_KEYS: &[(u8, u16, Action)] = &[
    (CMD, b'C' as u16, Action::Copy),
    (CMD, b'V' as u16, Action::Paste),
    (CMD, b'A' as u16, Action::SelectAll),
    (CMD, b'K' as u16, Action::ClearScrollback),
    (SHIFT, 0x21, Action::ScrollPage(1)),
    (SHIFT, 0x22, Action::ScrollPage(-1)),
    (CMD, 0x21, Action::ScrollPage(1)),
    (CMD, 0x22, Action::ScrollPage(-1)),
    (CMD, 0x24, Action::ScrollEnd(1)),
    (CMD, 0x23, Action::ScrollEnd(-1)),
    (CMD, b'T' as u16, Action::NewTab),
    (CMD, b'W' as u16, Action::ClosePane),
    (CMD | SHIFT, b'W' as u16, Action::CloseTab),
    (CMD | SHIFT, b'T' as u16, Action::ReopenClosed),
    (CTRL, 0x09, Action::CycleTab(1)),
    (CTRL | SHIFT, 0x09, Action::CycleTab(-1)),
    // Cmd+Shift+] and Cmd+Shift+[.
    (CMD | SHIFT, 0xdd, Action::CycleTab(1)),
    (CMD | SHIFT, 0xdb, Action::CycleTab(-1)),
    (CMD | SHIFT, 0x21, Action::MoveTab(-1)),
    (CMD | SHIFT, 0x22, Action::MoveTab(1)),
    (CMD, b'D' as u16, Action::SplitRight),
    (CMD | SHIFT, b'D' as u16, Action::SplitDown),
    (CMD | ALT, 0x25, Action::Focus(Dir::Left)),
    (CMD | ALT, 0x26, Action::Focus(Dir::Up)),
    (CMD | ALT, 0x27, Action::Focus(Dir::Right)),
    (CMD | ALT, 0x28, Action::Focus(Dir::Down)),
    (CMD | CTRL, 0x25, Action::Resize(Dir::Left)),
    (CMD | CTRL, 0x26, Action::Resize(Dir::Up)),
    (CMD | CTRL, 0x27, Action::Resize(Dir::Right)),
    (CMD | CTRL, 0x28, Action::Resize(Dir::Down)),
    (CMD | ALT | SHIFT, 0x25, Action::Swap(Dir::Left)),
    (CMD | ALT | SHIFT, 0x26, Action::Swap(Dir::Up)),
    (CMD | ALT | SHIFT, 0x27, Action::Swap(Dir::Right)),
    (CMD | ALT | SHIFT, 0x28, Action::Swap(Dir::Down)),
    (CMD | SHIFT, b'J' as u16, Action::JumpToAttention),
    (CMD | SHIFT, b'O' as u16, Action::GoToSession),
    (CMD | SHIFT, b'B' as u16, Action::ToggleSidebar),
    (CMD | SHIFT, b'U' as u16, Action::Update),
    (CMD | SHIFT, b'K' as u16, Action::ThemePicker),
    (CMD, 0xbc, Action::Settings),
    // Not Cmd+?: that is the Help menu's.
    (CMD, 0xbf, Action::Keys),
    (CMD | SHIFT, 0x0d, Action::Zoom),
    (CMD, 0xbb, Action::FontSize(1)),
    (CMD | SHIFT, 0xbb, Action::FontSize(1)),
    (CMD, 0xbd, Action::FontSize(-1)),
    (CMD, b'0' as u16, Action::FontSize(0)),
    (CMD | CTRL, b'F' as u16, Action::Fullscreen),
    (CMD | SHIFT, b'P' as u16, Action::Palette),
    (CMD, 0x26, Action::JumpToPrompt(1)),
    (CMD, 0x28, Action::JumpToPrompt(-1)),
    (CMD, b'F' as u16, Action::Find),
    (CMD | SHIFT, b'E' as u16, Action::Files),
    (CMD | SHIFT, 0x20, Action::QuickSelect),
];

/// The modifier that goes with a digit to pick a tab.
const TAB_DIGIT: u8 = if cfg!(target_os = "macos") { CMD } else { CTRL };

/// Key names for chords, matched ignoring case. The first name of each
/// key is the one the command palette shows. Letters, digits, F1 to F24
/// and Numpad0 to Numpad9 need no entry.
const KEY_NAMES: &[(&str, u16)] = &[
    ("Backspace", 0x08),
    ("Tab", 0x09),
    ("Enter", 0x0d),
    ("Esc", 0x1b),
    ("Escape", 0x1b),
    ("Space", 0x20),
    ("PgUp", 0x21),
    ("PageUp", 0x21),
    ("PgDn", 0x22),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Insert", 0x2d),
    ("Ins", 0x2d),
    ("Delete", 0x2e),
    ("Del", 0x2e),
    // The punctuation keys, by what they type on a US layout. Plus is the
    // key with = on it.
    (";", 0xba),
    ("Semicolon", 0xba),
    ("=", 0xbb),
    ("Equal", 0xbb),
    ("+", 0xbb),
    ("Plus", 0xbb),
    (",", 0xbc),
    ("Comma", 0xbc),
    ("-", 0xbd),
    ("Minus", 0xbd),
    (".", 0xbe),
    ("Period", 0xbe),
    ("/", 0xbf),
    ("Slash", 0xbf),
    ("`", 0xc0),
    ("Backquote", 0xc0),
    ("[", 0xdb),
    ("BracketLeft", 0xdb),
    ("\\", 0xdc),
    ("Backslash", 0xdc),
    ("]", 0xdd),
    ("BracketRight", 0xdd),
    ("'", 0xde),
    ("Quote", 0xde),
    // The keypad's other keys with Num Lock on.
    ("NumpadMultiply", 0x6a),
    ("NumpadAdd", 0x6b),
    ("NumpadSubtract", 0x6d),
    ("NumpadDecimal", 0x6e),
    ("NumpadDivide", 0x6f),
];

/// A binding as `config.toml` writes it: a chord, `=`, and an action's
/// name from [`ACTIONS`] or `none`, as in `ctrl+shift+r=split_right`. The
/// chord is any of `ctrl`, `shift`, `alt` and `cmd` (or `super`, the
/// Windows key), each once, and one key, joined by `+`. Case and spaces around the parts do not matter.
pub fn binding(s: &str) -> Option<Binding> {
    let (chord, name) = s.rsplit_once('=')?;
    let action = match name.trim() {
        n if n.eq_ignore_ascii_case("none") => None,
        n => Some(ACTIONS.iter().find(|a| a.1.eq_ignore_ascii_case(n))?.0),
    };
    let (mods, vk) = parse_chord(chord)?;
    Some((mods, vk, action))
}

/// A binding that types text, as in `ctrl+shift+e=text:git status\r`: the
/// chord, and the text with the escapes of [`unesc`]. Read before
/// [`binding`] would split at an `=` in the text.
pub fn text_binding(s: &str) -> Option<(u8, u16, Vec<u8>)> {
    let (chord, text) = s.split_once("=text:")?;
    let (mods, vk) = parse_chord(chord)?;
    Some((mods, vk, unesc(text))).filter(|b| !b.2.is_empty())
}

/// Text with the escapes `\e \r \n \t \s \\ \xNN`, as bytes. Any other
/// backslash stays.
pub fn unesc(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let (c, next) = (b[i], b.get(i + 1).copied());
        i += 1;
        if c != b'\\' || next.is_none() {
            out.push(c);
            continue;
        }
        i += 1;
        match next.unwrap_or_default() {
            b'e' => out.push(0x1b),
            b'r' => out.push(b'\r'),
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b's' => out.push(b' '),
            b'\\' => out.push(b'\\'),
            b'x' => match (s.get(i..i + 2))
                .filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                Some(v) => {
                    out.push(v);
                    i += 2;
                }
                None => out.extend_from_slice(b"\\x"),
            },
            other => out.extend_from_slice(&[b'\\', other]),
        }
    }
    out
}

/// The chord of a `keybind` value, whether it runs an action or types
/// text.
pub fn chord_of(line: &str) -> Option<Chord> {
    match text_binding(line) {
        Some((mods, vk, _)) => Some((mods, vk)),
        None => binding(line).map(|(mods, vk, _)| (mods, vk)),
    }
}

/// A chord's modifiers and virtual key.
fn parse_chord(chord: &str) -> Option<Chord> {
    let chord = chord.trim();
    // A + key ends the chord, after the + that joins it on, if any.
    let (mods, key) = match chord.strip_suffix('+').map(str::trim_end) {
        Some("") => ("", "+"),
        Some(mods) => (mods.strip_suffix('+')?, "+"),
        None => chord.rsplit_once('+').unwrap_or(("", chord)),
    };
    let mut bits = 0;
    for m in mods.split('+').filter(|_| !mods.is_empty()) {
        let bit = match m.trim().to_ascii_lowercase().as_str() {
            "ctrl" => CTRL,
            "shift" => SHIFT,
            "alt" | "opt" | "option" => ALT,
            "cmd" | "command" | "super" | "win" => CMD,
            _ => return None,
        };
        if bits & bit != 0 {
            return None;
        }
        bits |= bit;
    }
    Some((bits, key_code(key.trim())?))
}

/// The virtual key a key name stands for.
fn key_code(name: &str) -> Option<u16> {
    if let Some(&(_, vk)) = KEY_NAMES.iter().find(|k| k.0.eq_ignore_ascii_case(name)) {
        return Some(vk);
    }
    let f = (name.strip_prefix(['f', 'F']))
        .filter(|n| !n.starts_with('0'))
        .and_then(|n| n.parse::<u16>().ok());
    let pad = (name.get(..6).filter(|p| p.eq_ignore_ascii_case("numpad")))
        .and_then(|_| name[6..].parse::<u16>().ok());
    match (f, name.as_bytes()) {
        _ if pad.is_some_and(|n| n <= 9) => pad.map(|n| 0x60 + n),
        (Some(n @ 1..=24), _) => Some(0x6f + n),
        (_, &[c]) if c.is_ascii_alphanumeric() => Some(c.to_ascii_uppercase().into()),
        _ => None,
    }
}

/// A chord as the command palette shows it, such as `Ctrl+Shift+R`.
pub fn chord_label(mods: u8, vk: u16) -> String {
    let mut s = String::new();
    let cmd = if cfg!(target_os = "macos") {
        "Cmd+"
    } else if cfg!(windows) {
        "Win+"
    } else {
        "Super+"
    };
    let alt = if cfg!(target_os = "macos") {
        "Opt+"
    } else {
        "Alt+"
    };
    for (bit, name) in [(CTRL, "Ctrl+"), (ALT, alt), (SHIFT, "Shift+"), (CMD, cmd)] {
        if mods & bit != 0 {
            s.push_str(name);
        }
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5a => s.push(char::from(vk as u8)),
        0x60..=0x69 => s.push_str(&format!("Numpad{}", vk - 0x60)),
        0x70..=0x87 => s.push_str(&format!("F{}", vk - 0x6f)),
        _ => s.push_str(KEY_NAMES.iter().find(|k| k.1 == vk).map_or("?", |k| k.0)),
    }
    s
}

/// A chord as `config.toml` writes it, such as `ctrl+shift+/`, `win+x` or
/// `opt+cmd+left`, which [`binding`] reads back.
pub fn chord_text(c: Chord) -> String {
    chord_label(c.0, c.1).to_lowercase()
}

/// Why chord `c` cannot be a shortcut: a key alone, or with only Shift,
/// is the program's, and a key blitz has no name for cannot be written
/// in config.toml. F1 to F24 may go alone; arrows, Home, End, PgUp,
/// PgDn, Insert and Delete with Shift alone.
pub fn refused(c: Chord) -> Option<&'static str> {
    let (mods, vk) = c;
    let f = (0x70..=0x87).contains(&vk);
    if parse_chord(&chord_text(c)) != Some(c) {
        Some("blitz has no name for this key")
    } else if mods == 0 && !f {
        Some(if cfg!(target_os = "macos") {
            "A key alone goes to the program; hold Cmd or Ctrl with it"
        } else {
            "A key alone goes to the program; hold Ctrl or Alt with it"
        })
    } else if mods == SHIFT && !f && !matches!(vk, 0x21..=0x28 | 0x2d | 0x2e) {
        Some(if cfg!(target_os = "macos") {
            "Shift and this key go to the program; hold Cmd or Ctrl too"
        } else {
            "Shift and this key go to the program; hold Ctrl or Alt too"
        })
    } else {
        None
    }
}

/// Every binding in effect: the user's, then the defaults whose chords
/// they leave alone.
fn bindings(user: &[Binding]) -> impl Iterator<Item = Binding> + '_ {
    let defaults = (DEFAULT_KEYS.iter())
        .filter(|d| !user.iter().any(|u| (u.0, u.1) == (d.0, d.1)))
        .map(|&(m, vk, a)| (m, vk, Some(a)));
    user.iter().copied().chain(defaults)
}

/// The first chord that runs `a`, as the command palette shows it.
pub fn keys_for(a: Action, user: &[Binding]) -> Option<String> {
    bindings(user)
        .find(|b| b.2 == Some(a))
        .map(|(m, vk, _)| chord_label(m, vk))
}

/// How the command palette shows `a`.
pub fn label(a: Action) -> &'static str {
    ACTIONS.iter().find(|x| x.0 == a).map_or("", |x| x.2)
}

/// The name of `a` in `config.toml`.
pub fn name(a: Action) -> &'static str {
    ACTIONS.iter().find(|x| x.0 == a).map_or("", |x| x.1)
}

/// The digit key that goes to tab `a` with no binding for it.
fn tab_digit(a: Action) -> Option<Chord> {
    match a {
        Action::GoToTab(i @ 0..8) => Some((TAB_DIGIT, 0x31 + u16::from(i))),
        Action::LastTab => Some((TAB_DIGIT, 0x39)),
        _ => None,
    }
}

/// What chord `c` runs with no binding for it.
pub fn default_for(c: Chord) -> Option<Action> {
    match DEFAULT_KEYS.iter().find(|d| (d.0, d.1) == c) {
        Some(d) => Some(d.2),
        None => (c.0 == TAB_DIGIT && (0x31..=0x39).contains(&c.1)).then(|| match c.1 {
            0x39 => Action::LastTab,
            vk => Action::GoToTab((vk - 0x31) as u8),
        }),
    }
}

/// The chords that run `a` with no bindings.
pub fn defaults(a: Action) -> Vec<Chord> {
    (DEFAULT_KEYS.iter())
        .filter(|d| d.2 == a)
        .map(|d| (d.0, d.1))
        .chain(tab_digit(a))
        .collect()
}

/// Every chord that runs `a`, the `user` bindings' first.
pub fn chords(a: Action, user: &[Binding]) -> Vec<Chord> {
    let digit = tab_digit(a).filter(|&d| !user.iter().any(|u| (u.0, u.1) == d));
    (bindings(user))
        .filter(|b| b.2 == Some(a))
        .map(|b| (b.0, b.1))
        .chain(digit)
        .collect()
}

/// What chord `c` runs: its `user` binding's action, or `None` where that
/// gives it to the program, else its default.
pub fn bound_to(c: Chord, user: &[Binding]) -> Option<Action> {
    match user.iter().find(|u| (u.0, u.1) == c) {
        Some(u) => u.2,
        None => default_for(c),
    }
}

/// What a notice tells the user to press for `a`: its first chord, or
/// with none its name in the command palette.
pub fn press_for(a: Action, user: &[Binding]) -> String {
    keys_for(a, user).unwrap_or_else(|| format!("\"{}\"", label(a)))
}

/// The chord a key press makes: the modifiers held and the virtual key.
/// `None` for a release, and for a key AltGr (Ctrl+Alt) types with.
pub fn pressed(k: &KeyInput) -> Option<Chord> {
    if !k.down {
        return None;
    }
    let m = &k.mods;
    let held = [
        (m.lctrl || m.rctrl, CTRL),
        (m.lshift || m.rshift, SHIFT),
        (m.lalt || m.ralt, ALT),
        (m.lsuper || m.rsuper, CMD),
    ];
    let mods = held.iter().filter(|h| h.0).fold(0, |a, h| a | h.1);
    // Windows reports AltGr as Ctrl+Alt: a key it types a character with,
    // or a dead key that starts one, is typing, not a Ctrl+Alt chord.
    let typed = char::from_u32(k.uc.into()).is_some_and(|c| !c.is_control()) || k.key == Key::Other;
    if mods & (CTRL | ALT) == CTRL | ALT && typed {
        return None;
    }
    Some((mods, k.vk))
}

/// The shortcut a key press triggers, if any, with the `user` bindings
/// from `config.toml` over the defaults. Modifiers must match exactly, so
/// AltGr (Ctrl+Alt) and Win never trigger Ctrl shortcuts.
pub fn action(k: &KeyInput, user: &[Binding]) -> Option<Action> {
    let (mods, vk) = pressed(k)?;
    if let Some(b) = bindings(user).find(|b| (b.0, b.1) == (mods, vk)) {
        return b.2;
    }
    // Ctrl and a digit go to a tab, 9 to the last, where the key types
    // that digit. On AZERTY the same keys type & é " ' ( - è _ ç, and
    // Ctrl+- and Ctrl+_ are for the program. On a Mac it is Cmd, and any
    // digit key.
    let typed_digit = k.key == Key::Char(char::from(k.vk as u8));
    let digit = (0x31..=0x39).contains(&k.vk) && (typed_digit || TAB_DIGIT == CMD);
    (mods == TAB_DIGIT && digit).then(|| match k.vk {
        0x39 => Action::LastTab,
        vk => Action::GoToTab((vk - 0x31) as u8),
    })
}

/// Whether the key of a shortcut with nothing to do right now goes on to
/// the program. Update and Zoom keep theirs: under the default chords the
/// program would get ^U, which erases the line, and ^Z, which suspends.
pub fn passes_on(a: Action) -> bool {
    !matches!(a, Action::Update | Action::Zoom)
}

/// Whether a key message is an auto-repeat: lParam bit 30 says the key was
/// already down.
pub fn held_before(lparam: isize) -> bool {
    lparam as u32 & 1 << 30 != 0
}

/// Whether the auto-repeat of a held key does again what its press did,
/// when blitz took the press (`panel` when the theme picker or settings
/// panel did). Moving, scrolling, the font size and typing do. Anything
/// that confirms, toggles, opens or closes does not: a held key would
/// answer its own "press again", flicker, close pane after pane, or reach
/// the program once its press closed what took it.
pub fn repeats(k: &KeyInput, user: &[Binding], panel: bool) -> bool {
    match action(k, user) {
        Some(a) => matches!(
            a,
            Action::ScrollPage(_)
                | Action::CycleTab(_)
                | Action::MoveTab(_)
                | Action::Focus(_)
                | Action::Resize(_)
                | Action::Swap(_)
                | Action::FontSize(_)
        ),
        None => panel && !matches!(k.key, Key::Enter | Key::Escape | Key::Delete),
    }
}

/// Whether blitz drops the auto-repeat of a held key: the press was blitz's
/// (`taken`), and [`repeats`] says no. A shortcut that asks to be pressed
/// again counts as taken even when blitz lost track of its press, as when
/// focus left and came back while it was held, so a held key never answers
/// its own "press again". Other shortcuts that passed their press on, as
/// Ctrl+C with nothing selected, pass their repeats on too.
pub fn drops_repeat(k: &KeyInput, user: &[Binding], taken: bool, panel: bool) -> bool {
    let confirms = matches!(
        action(k, user),
        Some(Action::Paste | Action::ClosePane | Action::CloseTab | Action::Update)
    );
    (taken || confirms) && !repeats(k, user, panel)
}

/// Unshifted characters of a US layout by set-1 scan code, NUL where the
/// key types nothing.
const US_BY_SCAN: &[u8] =
    b"\0\x001234567890-=\0\0qwertyuiop[]\0\0asdfghjkl;'`\0\\zxcvbnm,./\0\0\0 ";

/// Turns a `WM_KEYDOWN`, `WM_KEYUP`, `WM_SYSKEYDOWN` or `WM_SYSKEYUP` into
/// key input for [`vt::encode_key`].
///
/// `keystate` is `GetKeyboardState` read while handling the message, so it
/// already includes this key's own transition. `layout(vk, scan, state)`
/// returns what the keyboard layout types for the key under `state`, as
/// `ToUnicodeEx` does without touching the dead key state, and `None` for a
/// dead key. A dead key sends nothing: what it composes comes as WM_CHAR
/// with the next key. The text of the result is stored in `text`.
pub fn msg_to_key<'a>(
    vk: u16,
    lparam: isize,
    keystate: &[u8; 256],
    layout: impl Fn(u16, u16, &[u8; 256]) -> Option<String>,
    text: &'a mut String,
) -> KeyInput<'a> {
    // Only the low 32 bits carry anything; a 64-bit LPARAM may be sign
    // extended for key-ups.
    let lp = lparam as u32;
    let scan = (lp >> 16 & 0xff) as u16;
    let extended = lp & 1 << 24 != 0;
    let held = |vk: usize| keystate[vk] & 0x80 != 0;
    let on = |vk: usize| keystate[vk] & 1 != 0;
    let mods = Mods {
        lshift: held(0xa0),
        rshift: held(0xa1),
        lctrl: held(0xa2),
        rctrl: held(0xa3),
        lalt: held(0xa4),
        ralt: held(0xa5),
        lsuper: held(0x5b),
        rsuper: held(0x5c),
    };
    let ctrl = mods.lctrl || mods.rctrl;
    let alt = mods.lalt || mods.ralt;

    let typed = layout(vk, scan, keystate);
    // An AltGr dead key types nothing either, even though the same key
    // without Ctrl and Alt does.
    let dead = typed.is_none();
    let typed = typed.unwrap_or_default();
    let uc = typed.encode_utf16().next().unwrap_or(0);
    // Windows reports AltGr as Ctrl+Alt. When Ctrl+Alt types something
    // printable that is the key's text; otherwise the text is what the key
    // types with Ctrl and Alt let go.
    *text = if dead || !(ctrl || alt) || ctrl && alt && printable(&typed) {
        typed
    } else {
        let mut plain = *keystate;
        for vk in [0x11, 0x12, 0xa2, 0xa3, 0xa4, 0xa5] {
            plain[vk] &= !0x80;
        }
        layout(vk, scan, &plain).unwrap_or_default()
    };
    if !printable(text) {
        text.clear();
    }

    let key = match vk {
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x0d => Key::Enter,
        0x1b => Key::Escape,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x23 => Key::End,
        0x24 => Key::Home,
        0x25 => Key::Left,
        0x26 => Key::Up,
        0x27 => Key::Right,
        0x28 => Key::Down,
        0x2d => Key::Insert,
        0x2e => Key::Delete,
        0x70..=0x87 => Key::F((vk - 0x6f) as u8),
        0x10 | 0xa0 | 0xa1 => Key::Shift,
        0x11 | 0xa2 | 0xa3 => Key::Control,
        0x12 | 0xa4 | 0xa5 => Key::Alt,
        0x5b | 0x5c => Key::Super,
        _ if dead => Key::Other,
        _ => {
            let base = layout(vk, scan, &[0; 256]).unwrap_or_default();
            let mut chars = base.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if !c.is_control() => Key::Char(c),
                // A key whose plain press is dead or empty still types
                // something with the modifiers held.
                _ => text.chars().next().map_or(Key::Other, Key::Char),
            }
        }
    };
    let us_base = US_BY_SCAN
        .get(usize::from(scan))
        .filter(|&&b| b != 0 && !extended)
        .map(|&b| char::from(b));

    KeyInput {
        vk,
        scan,
        extended,
        down: lp & 1 << 31 == 0,
        repeat: lp as u16,
        mods,
        locks: Locks {
            caps: on(0x14),
            num: on(0x90),
            scroll: on(0x91),
        },
        text,
        uc,
        cs: 0,
        key,
        us_base,
    }
}

fn printable(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(char::is_control)
}

/// The calling thread's keyboard layout, as the `layout` of [`msg_to_key`].
#[cfg(windows)]
pub fn system_layout(vk: u16, scan: u16, keystate: &[u8; 256]) -> Option<String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::ToUnicode;
    let mut buf = [0u16; 8];
    // Flag 4 leaves the dead key state alone, so the WM_CHAR that
    // TranslateMessage makes for this key still composes. A dead key
    // returns a negative count.
    // SAFETY: `keystate` and `buf` are valid for the whole call.
    let n = unsafe { ToUnicode(vk.into(), scan.into(), Some(keystate), &mut buf, 4) };
    let n = usize::try_from(n).ok()?.min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// The set-1 scan code of virtual key `vk` on a US keyboard, and whether
/// it is an extended key, for programs that read win32-input-mode.
pub fn scan_code(vk: u16) -> (u16, bool) {
    #[cfg(windows)]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};
        // SAFETY: a table lookup with no pointers involved.
        let scan = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) } as u16;
        (scan, matches!(vk, 0x21..=0x28 | 0x2d | 0x2e | 0x5b))
    }
    #[cfg(not(windows))]
    {
        let ext = |s: u16| (s, true);
        match vk {
            0x21 => ext(0x49),
            0x22 => ext(0x51),
            0x23 => ext(0x4f),
            0x24 => ext(0x47),
            0x25 => ext(0x4b),
            0x26 => ext(0x48),
            0x27 => ext(0x4d),
            0x28 => ext(0x50),
            0x2d => ext(0x52),
            0x2e => ext(0x53),
            0x5b => ext(0x5b),
            0x5c => ext(0x5c),
            0x5d => ext(0x5d),
            vk => (
                US_SCANS.iter().find(|s| s.0 == vk).map_or(0, |s| s.1),
                false,
            ),
        }
    }
}

/// Virtual keys and the set-1 scan codes of their keys on a US keyboard,
/// but for the extended keys.
#[cfg(not(windows))]
const US_SCANS: &[(u16, u16)] = &[
    (0x08, 0x0e),
    (0x09, 0x0f),
    (0x0d, 0x1c),
    (0x10, 0x2a),
    (0x11, 0x1d),
    (0x12, 0x38),
    (0x14, 0x3a),
    (0x1b, 0x01),
    (0x20, 0x39),
    (0x30, 0x0b),
    (0x31, 0x02),
    (0x32, 0x03),
    (0x33, 0x04),
    (0x34, 0x05),
    (0x35, 0x06),
    (0x36, 0x07),
    (0x37, 0x08),
    (0x38, 0x09),
    (0x39, 0x0a),
    (0x41, 0x1e),
    (0x42, 0x30),
    (0x43, 0x2e),
    (0x44, 0x20),
    (0x45, 0x12),
    (0x46, 0x21),
    (0x47, 0x22),
    (0x48, 0x23),
    (0x49, 0x17),
    (0x4a, 0x24),
    (0x4b, 0x25),
    (0x4c, 0x26),
    (0x4d, 0x32),
    (0x4e, 0x31),
    (0x4f, 0x18),
    (0x50, 0x19),
    (0x51, 0x10),
    (0x52, 0x13),
    (0x53, 0x1f),
    (0x54, 0x14),
    (0x55, 0x16),
    (0x56, 0x2f),
    (0x57, 0x11),
    (0x58, 0x2d),
    (0x59, 0x15),
    (0x5a, 0x2c),
    (0x60, 0x52),
    (0x61, 0x4f),
    (0x62, 0x50),
    (0x63, 0x51),
    (0x64, 0x4b),
    (0x65, 0x4c),
    (0x66, 0x4d),
    (0x67, 0x47),
    (0x68, 0x48),
    (0x69, 0x49),
    (0x6a, 0x37),
    (0x6b, 0x4e),
    (0x6d, 0x4a),
    (0x6e, 0x53),
    (0x6f, 0x35),
    (0x70, 0x3b),
    (0x71, 0x3c),
    (0x72, 0x3d),
    (0x73, 0x3e),
    (0x74, 0x3f),
    (0x75, 0x40),
    (0x76, 0x41),
    (0x77, 0x42),
    (0x78, 0x43),
    (0x79, 0x44),
    (0x7a, 0x57),
    (0x7b, 0x58),
    (0x90, 0x45),
    (0x91, 0x46),
    (0xba, 0x27),
    (0xbb, 0x0d),
    (0xbc, 0x33),
    (0xbd, 0x0c),
    (0xbe, 0x34),
    (0xbf, 0x35),
    (0xc0, 0x29),
    (0xdb, 0x1a),
    (0xdc, 0x2b),
    (0xdd, 0x1b),
    (0xde, 0x28),
    (0xe2, 0x56),
];

/// The modifier keys held, from winit's report of them. A side it does
/// not tell counts as the left.
#[cfg(not(windows))]
pub fn winit_mods(m: &winit::event::Modifiers) -> Mods {
    use winit::keyboard::ModifiersKeyState::Pressed;
    let s = m.state();
    let (rshift, rctrl, ralt, rsuper) = (
        m.rshift_state() == Pressed,
        m.rcontrol_state() == Pressed,
        m.ralt_state() == Pressed,
        m.rsuper_state() == Pressed,
    );
    Mods {
        lshift: s.shift_key() && (m.lshift_state() == Pressed || !rshift),
        rshift,
        lctrl: s.control_key() && (m.lcontrol_state() == Pressed || !rctrl),
        rctrl,
        lalt: s.alt_key() && (m.lalt_state() == Pressed || !ralt),
        ralt,
        lsuper: s.super_key() && (m.lsuper_state() == Pressed || !rsuper),
        rsuper,
    }
}

/// The virtual key of a physical key, by its place on a US keyboard.
#[cfg(not(windows))]
fn vk_of(code: winit::keyboard::KeyCode) -> u16 {
    use winit::keyboard::KeyCode as C;
    match code {
        C::Backquote => 0xc0,
        C::Backslash | C::IntlYen => 0xdc,
        C::BracketLeft => 0xdb,
        C::BracketRight => 0xdd,
        C::Comma => 0xbc,
        C::Digit0 => 0x30,
        C::Digit1 => 0x31,
        C::Digit2 => 0x32,
        C::Digit3 => 0x33,
        C::Digit4 => 0x34,
        C::Digit5 => 0x35,
        C::Digit6 => 0x36,
        C::Digit7 => 0x37,
        C::Digit8 => 0x38,
        C::Digit9 => 0x39,
        C::Equal => 0xbb,
        C::IntlBackslash | C::IntlRo => 0xe2,
        C::KeyA => 0x41,
        C::KeyB => 0x42,
        C::KeyC => 0x43,
        C::KeyD => 0x44,
        C::KeyE => 0x45,
        C::KeyF => 0x46,
        C::KeyG => 0x47,
        C::KeyH => 0x48,
        C::KeyI => 0x49,
        C::KeyJ => 0x4a,
        C::KeyK => 0x4b,
        C::KeyL => 0x4c,
        C::KeyM => 0x4d,
        C::KeyN => 0x4e,
        C::KeyO => 0x4f,
        C::KeyP => 0x50,
        C::KeyQ => 0x51,
        C::KeyR => 0x52,
        C::KeyS => 0x53,
        C::KeyT => 0x54,
        C::KeyU => 0x55,
        C::KeyV => 0x56,
        C::KeyW => 0x57,
        C::KeyX => 0x58,
        C::KeyY => 0x59,
        C::KeyZ => 0x5a,
        C::Minus => 0xbd,
        C::Period => 0xbe,
        C::Quote => 0xde,
        C::Semicolon => 0xba,
        C::Slash => 0xbf,
        C::AltLeft | C::AltRight => 0x12,
        C::Backspace => 0x08,
        C::CapsLock => 0x14,
        C::ContextMenu => 0x5d,
        C::ControlLeft | C::ControlRight => 0x11,
        C::Enter | C::NumpadEnter => 0x0d,
        C::SuperLeft => 0x5b,
        C::SuperRight => 0x5c,
        C::ShiftLeft | C::ShiftRight => 0x10,
        C::Space => 0x20,
        C::Tab => 0x09,
        C::Delete => 0x2e,
        C::End => 0x23,
        C::Home => 0x24,
        C::Insert => 0x2d,
        C::PageDown => 0x22,
        C::PageUp => 0x21,
        C::ArrowDown => 0x28,
        C::ArrowLeft => 0x25,
        C::ArrowRight => 0x27,
        C::ArrowUp => 0x26,
        C::NumLock => 0x90,
        C::Numpad0 => 0x60,
        C::Numpad1 => 0x61,
        C::Numpad2 => 0x62,
        C::Numpad3 => 0x63,
        C::Numpad4 => 0x64,
        C::Numpad5 => 0x65,
        C::Numpad6 => 0x66,
        C::Numpad7 => 0x67,
        C::Numpad8 => 0x68,
        C::Numpad9 => 0x69,
        C::NumpadAdd => 0x6b,
        C::NumpadDecimal | C::NumpadComma => 0x6e,
        C::NumpadDivide => 0x6f,
        C::NumpadMultiply => 0x6a,
        C::NumpadSubtract => 0x6d,
        C::Escape => 0x1b,
        C::F1 => 0x70,
        C::F2 => 0x71,
        C::F3 => 0x72,
        C::F4 => 0x73,
        C::F5 => 0x74,
        C::F6 => 0x75,
        C::F7 => 0x76,
        C::F8 => 0x77,
        C::F9 => 0x78,
        C::F10 => 0x79,
        C::F11 => 0x7a,
        C::F12 => 0x7b,
        C::F13 => 0x7c,
        C::F14 => 0x7d,
        C::F15 => 0x7e,
        C::F16 => 0x7f,
        C::F17 => 0x80,
        C::F18 => 0x81,
        C::F19 => 0x82,
        C::F20 => 0x83,
        C::F21 => 0x84,
        C::F22 => 0x85,
        C::F23 => 0x86,
        C::F24 => 0x87,
        C::PrintScreen => 0x2c,
        C::ScrollLock => 0x91,
        C::Pause => 0x13,
        _ => 0,
    }
}

/// What a winit key event says, for [`winit_key`].
#[cfg(not(windows))]
#[derive(Clone, Debug, Default)]
pub struct WinitKey {
    /// The physical key.
    pub code: Option<winit::keyboard::KeyCode>,
    /// What the key types on the layout with no modifiers held.
    pub base: Option<String>,
    /// The key's name, for keys that type nothing.
    pub named: Option<winit::keyboard::NamedKey>,
    /// What it types with every modifier held but Ctrl.
    pub text: Option<String>,
    pub down: bool,
    /// A dead key, which types nothing itself.
    pub dead: bool,
}

#[cfg(not(windows))]
impl WinitKey {
    pub fn of(ev: &winit::event::KeyEvent) -> WinitKey {
        use winit::keyboard::{Key as K, PhysicalKey};
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        WinitKey {
            code: match ev.physical_key {
                PhysicalKey::Code(c) => Some(c),
                PhysicalKey::Unidentified(_) => None,
            },
            base: match ev.key_without_modifiers() {
                K::Character(c) => Some(c.to_string()),
                _ => None,
            },
            named: match &ev.logical_key {
                K::Named(n) => Some(*n),
                _ => None,
            },
            text: ev.text.as_ref().map(ToString::to_string),
            down: ev.state == winit::event::ElementState::Pressed,
            dead: matches!(ev.logical_key, K::Dead(_)),
        }
    }
}

/// Turns a winit key event, with the modifiers `mods` held, into key
/// input for [`vt::encode_key`], as [`msg_to_key`] does a Windows key
/// message. The virtual key follows the label of a letter key on Latin
/// layouts and the US place of every other key, as on Windows. Option, or
/// Alt, that types a character of its own types it, as AltGr does; on a
/// key it changes nothing it is Meta. The text of the result is stored in
/// `text`.
#[cfg(not(windows))]
pub fn winit_key<'a>(k: &WinitKey, mut mods: Mods, text: &'a mut String) -> KeyInput<'a> {
    use winit::keyboard::NamedKey as N;
    let base = (k.base.as_deref())
        .and_then(|b| {
            let mut c = b.chars();
            c.next().filter(|_| c.next().is_none())
        })
        .filter(|c| !c.is_control());
    let physical = k.code.map_or(0, vk_of);
    let vk = match base {
        Some(c) if c.is_ascii_alphabetic() && !(0x30..=0x39).contains(&physical) => {
            u16::from(c.to_ascii_uppercase() as u8)
        }
        _ => physical,
    };
    let (scan, extended) = scan_code(physical);
    let extended = extended || matches!(k.code, Some(winit::keyboard::KeyCode::NumpadEnter));

    let typed = k.text.as_deref().filter(|t| printable(t)).unwrap_or("");
    let alt = mods.lalt || mods.ralt;
    let shifted = |c: char| c.to_uppercase().collect::<String>();
    // Option+e and the like: a character of its own, not Meta. Only a
    // Mac's Option types one: elsewhere Alt+Shift+1's `!` is Meta+!.
    let composed = cfg!(target_os = "macos")
        && alt
        && !typed.is_empty()
        && base.is_none_or(|b| typed != b.to_string() && typed != shifted(b));
    if composed {
        mods.lalt = false;
        mods.ralt = false;
    }
    *text = if k.dead { String::new() } else { typed.into() };
    let ctrl = mods.lctrl || mods.rctrl;
    let first = text.chars().next();
    // With Ctrl, a key gives its control code or none, as on Windows: a
    // printable one with Ctrl+Alt would read as AltGr typing it.
    let uc = match first {
        Some(c) if ctrl && (c.is_ascii_alphabetic() || ('@'..='_').contains(&c)) => c as u16 & 0x1f,
        Some(c) if ctrl && c != ' ' => 0,
        _ => text.encode_utf16().next().unwrap_or(0),
    };

    let key = match k.named {
        Some(N::Backspace) => Key::Backspace,
        Some(N::Tab) => Key::Tab,
        Some(N::Enter) => Key::Enter,
        Some(N::Escape) => Key::Escape,
        Some(N::PageUp) => Key::PageUp,
        Some(N::PageDown) => Key::PageDown,
        Some(N::End) => Key::End,
        Some(N::Home) => Key::Home,
        Some(N::ArrowLeft) => Key::Left,
        Some(N::ArrowUp) => Key::Up,
        Some(N::ArrowRight) => Key::Right,
        Some(N::ArrowDown) => Key::Down,
        Some(N::Insert) => Key::Insert,
        Some(N::Delete) => Key::Delete,
        Some(N::Shift) => Key::Shift,
        Some(N::Control) => Key::Control,
        Some(N::Alt | N::AltGraph) => Key::Alt,
        Some(N::Super | N::Meta) => Key::Super,
        Some(N::Space) => Key::Char(' '),
        _ if (0x70..=0x87).contains(&vk) => Key::F((vk - 0x6f) as u8),
        _ if k.dead => Key::Other,
        _ => base.or(first).map_or(Key::Other, Key::Char),
    };
    // winit does not say whether Caps Lock is on; a letter that comes out
    // in the other case than Shift asks for says it is.
    let caps = base.zip(first).is_some_and(|(b, t)| {
        let shift = mods.lshift || mods.rshift;
        b.is_alphabetic() && t.is_alphabetic() && t.is_uppercase() != shift && !composed
    });
    let us_base = US_BY_SCAN
        .get(usize::from(scan))
        .filter(|&&b| b != 0 && !extended)
        .map(|&b| char::from(b));
    KeyInput {
        vk,
        scan,
        extended,
        down: k.down,
        repeat: 1,
        mods,
        locks: Locks {
            caps,
            // winit does not say either; a keypad key that typed something
            // says it is on.
            num: (0x60..=0x6f).contains(&physical) && !typed.is_empty(),
            scroll: false,
        },
        text,
        uc,
        cs: 0,
        key,
        us_base,
    }
}

#[cfg(test)]
mod msg_to_key_tests {
    use super::*;

    const HELD: u8 = 0x80;

    /// Keyboard state with these left/right modifier keys held (the generic
    /// VK_SHIFT, VK_CONTROL and VK_MENU are set too, as Windows does) and
    /// these lock keys toggled on.
    fn state(held: &[usize], locks: &[usize]) -> [u8; 256] {
        let mut s = [0; 256];
        for &vk in held {
            s[vk] |= HELD;
            match vk {
                0xa0 | 0xa1 => s[0x10] |= HELD,
                0xa2 | 0xa3 => s[0x11] |= HELD,
                0xa4 | 0xa5 => s[0x12] |= HELD,
                _ => {}
            }
        }
        for &vk in locks {
            s[vk] |= 1;
        }
        s
    }

    /// lParam of a key message. Key-ups are sign extended, as they can be
    /// in a 64-bit LPARAM.
    fn lp(scan: u16, extended: bool, down: bool, repeat: u16) -> isize {
        let mut v = u32::from(repeat) | u32::from(scan) << 16 | u32::from(extended) << 24;
        if !down {
            v |= 3 << 30;
        }
        v as i32 as isize
    }

    /// Marks a dead key in a test layout.
    const DEAD: &str = "<dead>";

    /// A test layout from `(vk, plain, shifted, altgr)` rows. Ctrl alone
    /// turns letters and Enter into control codes, as Windows layouts do.
    fn layout(
        rows: &'static [(u16, &'static str, &'static str, &'static str)],
    ) -> impl Fn(u16, u16, &[u8; 256]) -> Option<String> {
        move |vk, _, s| {
            let held = |vk: usize| s[vk] & HELD != 0;
            let Some(&(_, plain, shifted, altgr)) = rows.iter().find(|r| r.0 == vk) else {
                return Some(String::new());
            };
            let caps = s[0x14] & 1 != 0 && plain.chars().all(char::is_alphabetic);
            let typed = match (held(0x11), held(0x12)) {
                (true, true) => altgr,
                (true, false) if (0x41..=0x5a).contains(&vk) => {
                    return Some(char::from(vk as u8 - 0x40).to_string());
                }
                (true, false) if vk == 0x0d => "\n",
                _ if held(0x10) != caps => shifted,
                _ => plain,
            };
            (typed != DEAD).then(|| typed.into())
        }
    }

    const US: &[(u16, &str, &str, &str)] = &[
        (0x0d, "\r", "\r", ""),
        (0x20, " ", " ", ""),
        (0x30, "0", ")", ""),
        (0x31, "1", "!", ""),
        (0x32, "2", "@", ""),
        (0x33, "3", "#", ""),
        (0x34, "4", "$", ""),
        (0x35, "5", "%", ""),
        (0x36, "6", "^", ""),
        (0x37, "7", "&", ""),
        (0x38, "8", "*", ""),
        (0x39, "9", "(", ""),
        (0x41, "a", "A", ""),
        (0x4a, "j", "J", ""),
        (0x51, "q", "Q", ""),
        (0x67, "7", "7", ""),
        (0x6e, ".", ".", ""),
    ];

    /// German: AltGr+Q is `@`, Z and Y swap places, the keypad has a comma.
    const DE: &[(u16, &str, &str, &str)] = &[
        (0x51, "q", "Q", "@"),
        (0x5a, "z", "Z", ""),
        (0x6e, ",", ",", ""),
    ];

    /// French AZERTY: the digit keys type symbols, and digits with Shift.
    #[cfg(not(target_os = "macos"))]
    const FR: &[(u16, &str, &str, &str)] = &[
        (0x31, "&", "1", ""),
        (0x36, "-", "6", "|"),
        (0x38, "_", "8", "\\"),
        (0x39, "\u{e7}", "9", "^"),
    ];

    /// Polish (214): AltGr+Q is a backslash.
    const PL: &[(u16, &str, &str, &str)] = &[(0x51, "q", "Q", "\\")];

    /// Czech: the 1 key types `+`, and AltGr+1 is a dead `~`. Shift+´ is a
    /// dead caron without AltGr.
    const CZ: &[(u16, &str, &str, &str)] = &[
        (0x31, "+", "1", DEAD),
        (0xbb, DEAD, DEAD, ""),
        (0x45, "e", "E", "\u{20ac}"),
    ];

    fn modes(kitty: u8, w32im: bool) -> vt::InputModes {
        vt::InputModes {
            kitty,
            w32im,
            ..Default::default()
        }
    }

    /// What the terminal sends for one key message.
    fn enc(
        vk: u16,
        lparam: isize,
        s: &[u8; 256],
        rows: &'static [(u16, &'static str, &'static str, &'static str)],
        m: &vt::InputModes,
    ) -> String {
        let mut t = String::new();
        let k = msg_to_key(vk, lparam, s, layout(rows), &mut t);
        let mut out = Vec::new();
        vt::encode_key(&k, m, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn keymap_shift_enter_fields() {
        let mut t = String::new();
        let k = msg_to_key(
            0x0d,
            lp(0x1c, false, true, 1),
            &state(&[0xa0], &[]),
            layout(US),
            &mut t,
        );
        let want = KeyInput {
            vk: 0x0d,
            scan: 0x1c,
            extended: false,
            down: true,
            repeat: 1,
            mods: Mods {
                lshift: true,
                ..Mods::default()
            },
            locks: Locks::default(),
            text: "",
            uc: 13,
            cs: 0,
            key: Key::Enter,
            us_base: None,
        };
        assert_eq!(k, want);
    }

    #[test]
    fn keymap_letters_text_and_control_codes() {
        let mut t = String::new();
        let k = msg_to_key(
            0x41,
            lp(0x1e, false, true, 1),
            &state(&[0xa1], &[]),
            layout(US),
            &mut t,
        );
        assert_eq!((k.text, k.uc, k.key), ("A", 'A' as u16, Key::Char('a')));
        assert!(k.mods.rshift && !k.mods.lshift);
        assert_eq!(k.us_base, Some('a'));

        // Caps Lock with Shift types lowercase again.
        let k = msg_to_key(
            0x41,
            lp(0x1e, false, true, 1),
            &state(&[0xa0], &[0x14]),
            layout(US),
            &mut t,
        );
        assert_eq!(k.text, "a");
        assert!(k.locks.caps);

        // Ctrl+J: the text ignores Ctrl, uc is the control code.
        let k = msg_to_key(
            0x4a,
            lp(0x24, false, true, 1),
            &state(&[0xa2], &[]),
            layout(US),
            &mut t,
        );
        assert_eq!((k.text, k.uc, k.key), ("j", 10, Key::Char('j')));
        assert!(k.mods.lctrl);
    }

    #[test]
    fn keymap_lparam_repeat_and_release() {
        let mut t = String::new();
        let k = msg_to_key(
            0x20,
            lp(0x39, false, true, 3),
            &[0; 256],
            layout(US),
            &mut t,
        );
        assert_eq!((k.down, k.repeat, k.text), (true, 3, " "));
        let k = msg_to_key(
            0x20,
            lp(0x39, false, false, 1),
            &[0; 256],
            layout(US),
            &mut t,
        );
        assert_eq!((k.down, k.repeat, k.scan), (false, 1, 0x39));
    }

    #[test]
    fn keymap_modifier_keys_and_locks() {
        let mut t = String::new();
        // Right Ctrl is VK_CONTROL with the extended bit.
        let k = msg_to_key(
            0x11,
            lp(0x1d, true, true, 1),
            &state(&[0xa3], &[0x90, 0x91]),
            layout(US),
            &mut t,
        );
        assert_eq!(
            (k.key, k.extended, k.uc, k.text),
            (Key::Control, true, 0, "")
        );
        assert!(k.mods.rctrl && !k.mods.lctrl);
        assert_eq!(
            (k.locks.num, k.locks.scroll, k.locks.caps),
            (true, true, false)
        );
        assert_eq!(k.us_base, None);
    }

    #[test]
    fn keymap_us_base_follows_the_key_position() {
        // German Z sits where US has Y.
        let mut t = String::new();
        let k = msg_to_key(
            0x5a,
            lp(0x15, false, true, 1),
            &[0; 256],
            layout(DE),
            &mut t,
        );
        assert_eq!((k.key, k.us_base), (Key::Char('z'), Some('y')));
    }

    #[test]
    fn keymap_keys_without_text() {
        let mut t = String::new();
        let k = msg_to_key(0x5d, lp(0x5d, true, true, 1), &[0; 256], layout(US), &mut t);
        assert_eq!((k.key, k.text, k.uc), (Key::Other, "", 0));
        let k = msg_to_key(
            0x7b,
            lp(0x58, false, true, 1),
            &[0; 256],
            layout(US),
            &mut t,
        );
        assert_eq!(k.key, Key::F(12));
    }

    #[test]
    fn keymap_shift_enter_with_kitty_flags() {
        // Num Lock stays out of the report unless all keys are escapes.
        let s = state(&[0xa0], &[0x90]);
        let enter = lp(0x1c, false, true, 1);
        assert_eq!(enc(0x0d, enter, &s, US, &modes(5, true)), "\x1b[13;2u");
    }

    #[test]
    fn keymap_shift_enter_in_win32_input_mode() {
        let m = modes(0, true);
        let shift = state(&[0xa0], &[]);
        let up = state(&[], &[]);
        let seq = [
            enc(0x10, lp(0x2a, false, true, 1), &shift, US, &m),
            enc(0x0d, lp(0x1c, false, true, 1), &shift, US, &m),
            enc(0x0d, lp(0x1c, false, false, 1), &shift, US, &m),
            enc(0x10, lp(0x2a, false, false, 1), &up, US, &m),
        ]
        .concat();
        assert_eq!(
            seq,
            "\x1b[16;42;0;1;16;1_\x1b[13;28;13;1;16;1_\x1b[13;28;13;0;16;1_\x1b[16;42;0;0;0;1_"
        );
    }

    #[test]
    fn keymap_altgr_sends_text_without_modifiers() {
        // AltGr is Left Ctrl plus Right Alt.
        let altgr = state(&[0xa2, 0xa5], &[]);
        let q = lp(0x10, false, true, 1);
        for m in [modes(0, false), modes(1, false), modes(5, false)] {
            assert_eq!(enc(0x51, q, &altgr, DE, &m), "@");
            assert_eq!(enc(0x51, q, &altgr, PL, &m), "\\");
        }
        // The console record keeps the real control state.
        assert_eq!(
            enc(0x51, q, &altgr, DE, &modes(0, true)),
            "\x1b[81;16;64;1;9;1_"
        );
        // A US layout has no AltGr, so Ctrl+Alt+Q stays a chord.
        assert_eq!(enc(0x51, q, &altgr, US, &modes(0, false)), "\x1b\x11");
        assert_eq!(enc(0x51, q, &altgr, US, &modes(1, false)), "\x1b[113;7u");
    }

    #[test]
    fn keymap_dead_keys_send_nothing() {
        let altgr = state(&[0xa2, 0xa5], &[]);
        let shift = state(&[0xa0], &[]);
        let one = lp(0x02, false, true, 1);
        for kitty in [0, 1, 5, 31] {
            let m = modes(kitty, false);
            assert_eq!(
                enc(0x31, one, &altgr, CZ, &m),
                "",
                "AltGr dead key, kitty {kitty}"
            );
            let up = lp(0x02, false, false, 1);
            assert_eq!(
                enc(0x31, up, &altgr, CZ, &m),
                "",
                "its release, kitty {kitty}"
            );
            let caron = lp(0x0d, false, true, 1);
            assert_eq!(
                enc(0xbb, caron, &shift, CZ, &m),
                "",
                "dead key, kitty {kitty}"
            );
        }
        let mut t = String::new();
        let k = msg_to_key(0x31, one, &altgr, layout(CZ), &mut t);
        assert_eq!((k.key, k.text, k.uc), (Key::Other, "", 0));
        // The console record still goes out, without a character.
        assert_eq!(
            enc(0x31, one, &altgr, CZ, &modes(0, true)),
            "\x1b[49;2;0;1;9;1_"
        );
        // The same key types as usual without AltGr, and so does a key
        // whose AltGr level is not dead.
        assert_eq!(enc(0x31, one, &[0; 256], CZ, &modes(0, false)), "+");
        let e = lp(0x12, false, true, 1);
        assert_eq!(enc(0x45, e, &altgr, CZ, &modes(0, false)), "\u{20ac}");
    }

    #[test]
    fn keymap_named_keys() {
        let mut t = String::new();
        let none = [0; 256];
        for (vk, scan, extended, want) in [
            (0x08, 0x0e, false, Key::Backspace),
            (0x09, 0x0f, false, Key::Tab),
            (0x1b, 0x01, false, Key::Escape),
            (0x21, 0x49, true, Key::PageUp),
            (0x22, 0x51, true, Key::PageDown),
            (0x23, 0x4f, true, Key::End),
            (0x25, 0x4b, true, Key::Left),
            (0x27, 0x4d, true, Key::Right),
            (0x28, 0x50, true, Key::Down),
            (0x2d, 0x52, true, Key::Insert),
            (0x2e, 0x53, true, Key::Delete),
            (0x70, 0x3b, false, Key::F(1)),
            (0x87, 0x76, false, Key::F(24)),
            (0x10, 0x2a, false, Key::Shift),
            (0xa1, 0x36, false, Key::Shift),
            (0xa2, 0x1d, false, Key::Control),
            (0x12, 0x38, false, Key::Alt),
            (0xa5, 0x38, true, Key::Alt),
            (0x5b, 0x5b, true, Key::Super),
            (0x5c, 0x5c, true, Key::Super),
        ] {
            let k = msg_to_key(vk, lp(scan, extended, true, 1), &none, layout(US), &mut t);
            assert_eq!((k.key, k.text, k.extended), (want, "", extended), "{vk:#x}");
        }
        // Ctrl+Enter types a line feed on Windows layouts; the text stays
        // what the key types without Ctrl, and uc is the control code.
        let k = msg_to_key(
            0x0d,
            lp(0x1c, false, true, 1),
            &state(&[0xa2], &[]),
            layout(US),
            &mut t,
        );
        assert_eq!((k.key, k.text, k.uc), (Key::Enter, "", 10));
    }

    #[test]
    fn keymap_numpad() {
        let mut t = String::new();
        // Num Lock on: the keypad types digits, and a comma on German.
        let num = state(&[], &[0x90]);
        let k = msg_to_key(0x67, lp(0x47, false, true, 1), &num, layout(US), &mut t);
        assert_eq!(
            (k.key, k.text, k.extended, k.us_base),
            (Key::Char('7'), "7", false, None)
        );
        assert!(k.locks.num);
        let k = msg_to_key(0x6e, lp(0x53, false, true, 1), &num, layout(DE), &mut t);
        assert_eq!((k.key, k.text), (Key::Char(','), ","));
        // Num Lock off: keypad 7 is Home without the extended bit; the Home
        // key has it.
        let k = msg_to_key(
            0x24,
            lp(0x47, false, true, 1),
            &[0; 256],
            layout(US),
            &mut t,
        );
        assert_eq!((k.key, k.extended), (Key::Home, false));
        let k = msg_to_key(0x24, lp(0x47, true, true, 1), &[0; 256], layout(US), &mut t);
        assert_eq!((k.key, k.extended), (Key::Home, true));
    }

    #[test]
    fn keymap_enhanced_bit() {
        let none = [0; 256];
        let kp_enter = lp(0x1c, true, true, 1);
        assert_eq!(
            enc(0x0d, kp_enter, &none, US, &modes(0, true)),
            "\x1b[13;28;13;1;256;1_"
        );
        // Keypad Enter has its own kitty code only when all keys are escapes.
        assert_eq!(
            enc(0x0d, kp_enter, &none, US, &modes(9, false)),
            "\x1b[57414u"
        );
        assert_eq!(enc(0x0d, kp_enter, &none, US, &modes(1, false)), "\r");
        // The arrow keys outside the keypad carry it too.
        assert_eq!(
            enc(0x26, lp(0x48, true, true, 1), &none, US, &modes(0, true)),
            "\x1b[38;72;0;1;256;1_"
        );
    }

    fn press(vk: u16, held: &[usize]) -> Option<Action> {
        let mut t = String::new();
        let k = msg_to_key(
            vk,
            lp(0, false, true, 1),
            &state(held, &[]),
            layout(US),
            &mut t,
        );
        action(&k, &[])
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keymap_default_shortcuts() {
        const LCTRL: usize = 0xa2;
        const LSHIFT: usize = 0xa0;
        const LALT: usize = 0xa4;
        assert_eq!(press(0x43, &[LCTRL]), Some(Action::Copy));
        assert_eq!(press(0x56, &[LCTRL, LSHIFT]), Some(Action::Paste));
        assert_eq!(press(0x2d, &[0xa1]), Some(Action::Paste));
        assert_eq!(press(0x21, &[LSHIFT]), Some(Action::ScrollPage(1)));
        assert_eq!(press(0x24, &[LCTRL, LSHIFT]), Some(Action::ScrollEnd(1)));
        assert_eq!(press(0x23, &[LCTRL, LSHIFT]), Some(Action::ScrollEnd(-1)));
        assert_eq!(press(0x23, &[LSHIFT]), None, "PSReadLine selects");
        assert_eq!(press(0x52, &[LCTRL, LSHIFT]), Some(Action::SplitRight));
        assert_eq!(press(0x44, &[0xa3, LSHIFT]), Some(Action::SplitDown));
        assert_eq!(press(0x42, &[LCTRL, LSHIFT]), Some(Action::ToggleSidebar));
        assert_eq!(press(0x4f, &[LCTRL, LSHIFT]), Some(Action::GoToSession));
        assert_eq!(press(0x09, &[LCTRL, LSHIFT]), Some(Action::CycleTab(-1)));
        assert_eq!(press(0x21, &[LCTRL, LSHIFT]), Some(Action::MoveTab(-1)));
        assert_eq!(press(0x22, &[LCTRL, LSHIFT]), Some(Action::MoveTab(1)));
        assert_eq!(press(0x33, &[LCTRL]), Some(Action::GoToTab(2)));
        assert_eq!(press(0x25, &[LCTRL, LALT]), Some(Action::Focus(Dir::Left)));
        assert_eq!(press(0x26, &[LALT, LSHIFT]), Some(Action::Resize(Dir::Up)));
        assert_eq!(
            press(0x27, &[LCTRL, LALT, LSHIFT]),
            Some(Action::Swap(Dir::Right))
        );
        assert_eq!(press(0x5a, &[LCTRL, LSHIFT]), Some(Action::Zoom));
        // Modifiers match exactly.
        assert_eq!(press(0x25, &[LCTRL, LSHIFT]), None, "selects a word");
        assert_eq!(press(0xbd, &[LCTRL]), Some(Action::FontSize(-1)));
        assert_eq!(press(0xbd, &[LCTRL, LSHIFT]), None, "Claude Code's undo");
        assert_eq!(press(0xbf, &[LCTRL, LSHIFT]), Some(Action::Keys));
        assert_eq!(press(0xbf, &[LCTRL]), None, "Claude Code's undo too");
        // Ctrl and + with Shift, as on a US layout, and on the keypad.
        assert_eq!(press(0xbb, &[LCTRL, LSHIFT]), Some(Action::FontSize(1)));
        assert_eq!(press(0x6b, &[LCTRL]), Some(Action::FontSize(1)));
        assert_eq!(press(0x6d, &[LCTRL]), Some(Action::FontSize(-1)));
        assert_eq!(press(0x60, &[LCTRL]), Some(Action::FontSize(0)));
        assert_eq!(press(0x30, &[LCTRL]), Some(Action::FontSize(0)));
        assert_eq!(press(0x52, &[LCTRL]), None);
        assert_eq!(press(0x43, &[LCTRL, LALT]), None, "AltGr is not Ctrl");
        assert_eq!(press(0x43, &[LCTRL, 0x5b]), None);
        assert_eq!(
            press(0x0d, &[LSHIFT]),
            None,
            "Shift+Enter goes to the program"
        );
        assert_eq!(press(0x22, &[]), None);
        // Releases never trigger.
        let mut t = String::new();
        let up = lp(0, false, false, 1);
        let k = msg_to_key(0x43, up, &state(&[LCTRL], &[]), layout(US), &mut t);
        assert_eq!(action(&k, &[]), None);
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keymap_reads_bindings() {
        assert_eq!(
            binding("ctrl+shift+r=split_right"),
            Some((CTRL | SHIFT, 0x52, Some(Action::SplitRight)))
        );
        assert_eq!(
            binding(" Alt + F11 = new_tab "),
            Some((ALT, 0x7a, Some(Action::NewTab)))
        );
        assert_eq!(
            binding("ctrl+shift+w=none"),
            Some((CTRL | SHIFT, 0x57, None))
        );
        assert_eq!(
            binding("cmd+shift+]=next_tab"),
            binding("Super+Shift+BracketRight=next_tab")
        );
        assert_eq!(binding("cmd+d=split_right").map(|b| b.0), Some(CMD));
        assert_eq!(
            binding("ctrl+shift+q=close_tab"),
            Some((CTRL | SHIFT, 0x51, Some(Action::CloseTab)))
        );
        assert_eq!(
            binding("ctrl+shift+o=reopen_closed"),
            Some((CTRL | SHIFT, 0x4f, Some(Action::ReopenClosed)))
        );
        // Punctuation by character or by name.
        let plus = Some((CTRL, 0xbb, Some(Action::Copy)));
        for s in [
            "ctrl+==copy",
            "ctrl++=copy",
            "ctrl+plus=copy",
            "CTRL+Equal=copy",
        ] {
            assert_eq!(binding(s), plus, "{s}");
        }
        assert_eq!(binding("ctrl+,=copy"), binding("ctrl+comma=copy"));
        assert_eq!(binding("shift+pgup=copy"), binding("shift+PageUp=copy"));
        assert_eq!(binding("7=copy"), Some((0, 0x37, Some(Action::Copy))));
        // Case and spaces do not matter, around a + key either.
        assert_eq!(
            binding("Ctrl+E=Split_Right"),
            Some((CTRL, 0x45, Some(Action::SplitRight)))
        );
        assert_eq!(
            binding("ctrl+shift+r=None"),
            Some((CTRL | SHIFT, 0x52, None))
        );
        assert_eq!(binding("ctrl + + = copy"), plus);
        assert_eq!(binding("+=copy"), Some((0, 0xbb, Some(Action::Copy))));
        for bad in [
            "ctrl+shift+r",
            "ctrl+shift+r=split_sideways",
            "ctrl+f25=copy",
            "ctrl+caps=copy",
            "=copy",
            "ctrl+=copy",
            "ctrl++a=copy",
            "shift+ctrl+shift+a=copy",
            "a+b=copy",
            "f01=copy",
        ] {
            assert_eq!(binding(bad), None, "{bad}");
        }
        // The keypad by name.
        assert_eq!(
            binding("ctrl+numpad0=copy"),
            Some((CTRL, 0x60, Some(Action::Copy)))
        );
        assert_eq!(
            binding("ctrl+Numpad9=copy"),
            Some((CTRL, 0x69, Some(Action::Copy)))
        );
        assert_eq!(
            binding("ctrl+numpadadd=copy"),
            Some((CTRL, 0x6b, Some(Action::Copy)))
        );
        assert_eq!(binding("ctrl+numpad10=copy"), None);
        assert_eq!(chord_label(CTRL, 0x60), "Ctrl+Numpad0");
        assert_eq!(chord_label(CTRL, 0x6d), "Ctrl+NumpadSubtract");
        assert_eq!(chord_label(CTRL | ALT | SHIFT, 0x25), "Ctrl+Alt+Shift+Left");
        assert_eq!(chord_label(CTRL, 0xbc), "Ctrl+,");
        assert_eq!(chord_label(0, 0x7a), "F11");
    }

    #[test]
    fn keymap_reads_text_bindings() {
        assert_eq!(
            text_binding(r"ctrl+shift+e=text:git status\r"),
            Some((CTRL | SHIFT, 0x45, b"git status\r".to_vec()))
        );
        // An = in the text, or as the key, is kept apart from the chord.
        assert_eq!(
            text_binding(r"alt+==text:a=b\e[A\x41\s"),
            Some((ALT, 0xbb, b"a=b\x1b[AA ".to_vec()))
        );
        // Only two hex digits make a byte.
        assert_eq!(unesc(r"\x+f\x4"), br"\x+f\x4");
        assert_eq!(text_binding("ctrl+e=text:"), None, "nothing to type");
        assert_eq!(text_binding("ctrl+bogus+e=text:x"), None);
        assert_eq!(text_binding("ctrl+e=split_right"), None);
        assert_eq!(binding(r"ctrl+e=text:x"), None, "not an action");
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keymap_user_bindings_come_before_the_defaults() {
        const LCTRL: usize = 0xa2;
        const LSHIFT: usize = 0xa0;
        let user: Vec<Binding> = ["ctrl+shift+r=new_tab", "ctrl+shift+w=none", "ctrl+1=none"]
            .into_iter()
            .filter_map(binding)
            .collect();
        let press = |vk, held: &[usize]| {
            let mut t = String::new();
            let lp = lp(0, false, true, 1);
            let k = msg_to_key(vk, lp, &state(held, &[]), layout(US), &mut t);
            action(&k, &user)
        };
        assert_eq!(press(0x52, &[LCTRL, LSHIFT]), Some(Action::NewTab));
        assert_eq!(press(0x54, &[LCTRL, LSHIFT]), Some(Action::NewTab));
        assert_eq!(press(0x57, &[LCTRL, LSHIFT]), None, "unbound");
        assert_eq!(press(0x31, &[LCTRL]), None, "Ctrl+1 unbound");
        assert_eq!(press(0x32, &[LCTRL]), Some(Action::GoToTab(1)));
        // The user's chord shows first, and an action whose only chord was
        // taken or unbound shows none.
        let keys = |a| keys_for(a, &user);
        assert_eq!(keys(Action::NewTab).as_deref(), Some("Ctrl+Shift+R"));
        assert_eq!(keys(Action::SplitRight), None);
        assert_eq!(keys(Action::ClosePane), None);
        assert_eq!(keys_for(Action::Copy, &[]).as_deref(), Some("Ctrl+C"));
        // A notice names the user's keys, or the palette's name.
        assert_eq!(press_for(Action::NewTab, &user), "Ctrl+Shift+R");
        assert_eq!(press_for(Action::ClosePane, &user), "\"Close pane\"");
        assert_eq!(press_for(Action::Paste, &[]), "Ctrl+V");
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keymap_ctrl_digits_go_to_tabs_where_the_key_types_the_digit() {
        let press_on = |vk, rows, user: &[Binding]| {
            let mut t = String::new();
            let lp = lp(0x09, false, true, 1);
            let k = msg_to_key(vk, lp, &state(&[0xa2], &[]), layout(rows), &mut t);
            action(&k, user)
        };
        assert_eq!(press_on(0x31, US, &[]), Some(Action::GoToTab(0)));
        assert_eq!(press_on(0x38, US, &[]), Some(Action::GoToTab(7)));
        assert_eq!(press_on(0x39, US, &[]), Some(Action::LastTab));
        // AZERTY: Ctrl+_ and Ctrl+- go to the program, and so do the rest.
        for vk in [0x31, 0x36, 0x38, 0x39] {
            assert_eq!(press_on(vk, FR, &[]), None, "{vk:#x}");
        }
        // Bound by name, a digit key goes to its tab on any layout.
        let user: Vec<Binding> = ["ctrl+1=go_to_tab_1", "ctrl+9=last_tab"]
            .into_iter()
            .filter_map(binding)
            .collect();
        assert_eq!(press_on(0x31, FR, &user), Some(Action::GoToTab(0)));
        assert_eq!(press_on(0x39, FR, &user), Some(Action::LastTab));
        assert_eq!(
            binding("alt+3=go_to_tab_3"),
            Some((ALT, 0x33, Some(Action::GoToTab(2))))
        );
        assert_eq!(binding("ctrl+0=go_to_tab_9"), None, "Ctrl+9 is the last");
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keymap_altgr_types_rather_than_running_a_ctrl_alt_binding() {
        let user: Vec<Binding> = [
            "ctrl+alt+q=new_tab",
            "ctrl+alt+1=new_tab",
            "ctrl+alt+left=split_right",
        ]
        .into_iter()
        .filter_map(binding)
        .collect();
        let press = |vk, rows, held: &[usize]| {
            let mut t = String::new();
            let lp = lp(0x10, false, true, 1);
            let k = msg_to_key(vk, lp, &state(held, &[]), layout(rows), &mut t);
            action(&k, &user)
        };
        let altgr = [0xa2, 0xa5];
        assert_eq!(press(0x51, DE, &altgr), None, "AltGr+Q types @");
        assert_eq!(press(0x51, PL, &altgr), None, "AltGr+Q types \\");
        assert_eq!(press(0x31, CZ, &altgr), None, "AltGr+1 is a dead ~");
        // Without a character for AltGr it is the chord.
        assert_eq!(press(0x51, US, &altgr), Some(Action::NewTab));
        assert_eq!(press(0x25, DE, &altgr), Some(Action::SplitRight), "arrows");
        // The default Ctrl+Alt+Arrows still move focus on AltGr layouts.
        let k = |vk| {
            let mut t = String::new();
            let lp = lp(0x4b, true, true, 1);
            let k = msg_to_key(vk, lp, &state(&altgr, &[]), layout(DE), &mut t);
            action(&k, &[])
        };
        assert_eq!(k(0x25), Some(Action::Focus(Dir::Left)));
    }

    /// The left-hand keys for a set of shortcut modifiers.
    fn held_for(m: u8) -> Vec<usize> {
        [(CTRL, 0xa2), (SHIFT, 0xa0), (ALT, 0xa4), (CMD, 0x5b)]
            .into_iter()
            .filter(|&(bit, _)| m & bit != 0)
            .map(|(_, vk)| vk)
            .collect()
    }

    #[test]
    fn keymap_every_default_shortcut_fires() {
        for &(m, vk, a) in DEFAULT_KEYS {
            assert_eq!(press(vk, &held_for(m)), Some(a), "{m} {vk:#x}");
            // The right-hand modifier keys count the same.
            let right: Vec<usize> = held_for(m).into_iter().map(|k| k + 1).collect();
            assert_eq!(press(vk, &right), Some(a), "right-hand {m} {vk:#x}");
            // One modifier more is another chord.
            for (bit, extra) in [(CTRL, 0xa2), (SHIFT, 0xa0), (ALT, 0xa4), (CMD, 0x5b)] {
                let mut more = held_for(m);
                if !more.contains(&extra) {
                    more.push(extra);
                    let other = (DEFAULT_KEYS.iter())
                        .find(|b| bit != 0 && b.0 == m | bit && b.1 == vk)
                        .map(|b| b.2);
                    assert_eq!(press(vk, &more), other, "{m} {vk:#x} + {extra:#x}");
                }
            }
        }
        // Ctrl, or Cmd on a Mac.
        let tab = if TAB_DIGIT == CMD { 0x5b } else { 0xa2 };
        for i in 0..8 {
            assert_eq!(press(0x31 + i, &[tab]), Some(Action::GoToTab(i as u8)));
        }
        assert_eq!(press(0x39, &[tab]), Some(Action::LastTab), "Ctrl+9");
        assert_eq!(press(0x30, &[tab]), Some(Action::FontSize(0)), "Ctrl+0");
        assert_eq!(press(0x31, &[tab, 0xa0]), None, "Ctrl+Shift+1");
        assert_eq!(press(0x31, &[tab, 0xa4]), None, "AltGr+1");
        assert_eq!(press(0x61, &[tab]), None, "Ctrl+keypad 1 is not tab 1");
    }

    #[test]
    fn keymap_default_shortcuts_are_unique_and_unshadowed() {
        for (i, a) in DEFAULT_KEYS.iter().enumerate() {
            for b in &DEFAULT_KEYS[i + 1..] {
                assert!((a.0, a.1) != (b.0, b.1), "{a:?} and {b:?}");
            }
            let tab = a.0 == TAB_DIGIT && (0x31..=0x39).contains(&a.1);
            assert!(!tab, "{a:?} is a Go to tab key");
        }
    }

    #[test]
    fn keymap_chord_text_reads_back() {
        for m in 0..16 {
            for vk in 0..=0xff {
                if chord_label(m, vk).contains('?') {
                    continue;
                }
                let text = chord_text((m, vk));
                let read = |name| binding(&format!("{text}={name}"));
                assert_eq!(read("copy"), Some((m, vk, Some(Action::Copy))), "{text}");
                assert_eq!(read("none"), Some((m, vk, None)), "{text}");
            }
        }
    }

    #[test]
    fn keymap_pressed_is_the_chord_of_a_key() {
        let chord = |vk, down, rows, held: &[usize]| {
            let mut t = String::new();
            let lp = lp(0, false, down, 1);
            let k = msg_to_key(vk, lp, &state(held, &[]), layout(rows), &mut t);
            pressed(&k)
        };
        let cs = [0xa2, 0xa0];
        assert_eq!(chord(0x52, true, US, &cs), Some((CTRL | SHIFT, 0x52)));
        assert_eq!(chord(0x52, false, US, &cs), None, "a release");
        let altgr = [0xa2, 0xa5];
        assert_eq!(chord(0x51, true, DE, &altgr), None, "AltGr+Q types @");
        assert_eq!(chord(0x25, true, DE, &altgr), Some((CTRL | ALT, 0x25)));
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keymap_chords_list_every_key_of_an_action() {
        let user =
            |lines: &[&str]| -> Vec<Binding> { lines.iter().filter_map(|l| binding(l)).collect() };
        let copy = [(CTRL, 0x43), (CTRL | SHIFT, 0x43), (CTRL, 0x2d)];
        assert_eq!(chords(Action::Copy, &[]), copy);
        assert_eq!(chords(Action::GoToTab(2), &[]), [(CTRL, 0x33)]);
        assert!(chords(Action::GoToTab(2), &user(&["ctrl+3=none"])).is_empty());
        assert!(chords(Action::CopyUnindented, &[]).is_empty());
        // The user's come first.
        assert_eq!(chords(Action::Copy, &user(&["alt+c=copy"]))[0], (ALT, 0x43));
        assert_eq!(defaults(Action::LastTab), [(CTRL, 0x39)]);
        assert_eq!(defaults(Action::Copy), copy);
        let u = user(&["ctrl+shift+t=split_right", "ctrl+shift+w=none"]);
        assert!(chords(Action::NewTab, &u).is_empty());
        assert_eq!(bound_to((CTRL | SHIFT, 0x54), &u), Some(Action::SplitRight));
        assert_eq!(bound_to((CTRL | SHIFT, 0x57), &u), None, "given back");
        assert_eq!(bound_to((CTRL, 0x43), &u), Some(Action::Copy));
        assert_eq!(bound_to((CTRL, 0x31), &u), Some(Action::GoToTab(0)));
        assert_eq!(bound_to((CTRL, 0x52), &u), None, "never bound");
        assert_eq!(default_for((CTRL, 0x39)), Some(Action::LastTab));
        // The chord of a line, whatever it binds.
        assert_eq!(
            chord_of(r"ctrl+shift+g=text:git status\r"),
            Some((CTRL | SHIFT, 0x47))
        );
        assert_eq!(chord_of("ctrl+shift+r=none"), Some((CTRL | SHIFT, 0x52)));
        assert_eq!(chord_of("ctrl+shift+r=quit"), None);
        assert_eq!(name(Action::SplitRight), "split_right");
    }

    #[test]
    fn keymap_refuses_keys_that_are_typing() {
        for c in [
            (0, 0x41),
            (SHIFT, 0x41),
            (SHIFT, 0x0d),
            (SHIFT, 0x09),
            (0, 0x1b),
        ] {
            assert!(refused(c).is_some(), "{c:?}");
        }
        let unnamed = refused((CTRL, 0x10));
        assert_eq!(unnamed, Some("blitz has no name for this key"));
        for c in [(SHIFT, 0x21), (0, 0x7a), (CTRL, 0x41), (ALT, 0x20)] {
            assert_eq!(refused(c), None, "{c:?}");
        }
        for &(m, vk, a) in DEFAULT_KEYS {
            assert_eq!(refused((m, vk)), None, "{a:?}");
        }
    }

    #[test]
    fn readme_lists_every_default_shortcut() {
        let readme = include_str!("../../../README.md");
        // Followed by something other than more of a key name, so
        // Ctrl+Shift+T does not count as Ctrl+Shift+Tab.
        let listed = |chord: &str| {
            readme.match_indices(chord).any(|(i, _)| {
                (readme[i + chord.len()..].chars().next())
                    .is_none_or(|c| !c.is_ascii_alphanumeric())
            })
        };
        for &(m, vk, _) in DEFAULT_KEYS {
            let chord = chord_label(m, vk);
            // Arrow keys may be listed together, as Ctrl+Alt+Arrows.
            let mods = chord_label(m, b'A' as u16);
            let arrows = format!("{}Arrows", mods.trim_end_matches('A'));
            let arrow = (0x25..=0x28).contains(&vk) && listed(&arrows);
            assert!(listed(&chord) || arrow, "README.md does not list {chord}");
        }
    }

    #[test]
    fn readme_names_every_action() {
        let readme = include_str!("../../../README.md");
        for &(a, name, _) in ACTIONS {
            // Listed as `go_to_tab_1` to `go_to_tab_8`.
            let between = matches!(a, Action::GoToTab(1..=6));
            let named = readme.contains(&format!("`{name}`"));
            assert!(named || between, "README.md does not name {name}");
        }
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keymap_auto_repeat() {
        assert!(!held_before(lp(0x16, false, true, 1)), "a first press");
        assert!(held_before(lp(0x16, false, true, 1) | 1 << 30), "a repeat");
        let repeats_with = |vk: u16, held: &[usize], panel: bool| {
            let mut t = String::new();
            let again = lp(0, false, true, 1) | 1 << 30;
            let k = msg_to_key(vk, again, &state(held, &[]), layout(US), &mut t);
            repeats(&k, &[], panel)
        };
        const CS: &[usize] = &[0xa2, 0xa0];
        // Holding a key must not answer its own "press again", flicker a
        // toggle, or open and close things over and over.
        for (vk, held) in [
            (0x55, CS),      // Update
            (0x57, CS),      // ClosePane
            (0x56, &[0xa2]), // Paste
            (0x43, &[0xa2]), // Copy
            (0x4b, CS),      // ThemePicker
            (0xbc, &[0xa2]), // Settings
            (0x42, CS),      // ToggleSidebar
            (0x54, CS),      // NewTab
            (0x52, CS),      // SplitRight
            (0x4a, CS),      // JumpToAttention
            (0x33, &[0xa2]), // GoToTab
        ] {
            for panel in [false, true] {
                assert!(!repeats_with(vk, held, panel), "{vk:#x} panel {panel}");
            }
        }
        // Moving, scrolling and the font size go on while the key is held.
        for (vk, held) in [
            (0x21, &[0xa0][..]),         // ScrollPage
            (0x09, &[0xa2]),             // CycleTab
            (0x22, &[0xa2, 0xa0]),       // MoveTab
            (0x25, &[0xa2, 0xa4]),       // Focus
            (0x26, &[0xa4, 0xa0]),       // Resize
            (0x27, &[0xa2, 0xa4, 0xa0]), // Swap
            (0xbb, &[0xa2]),             // FontSize
        ] {
            assert!(repeats_with(vk, held, false), "{vk:#x}");
        }
        // In a panel the arrows, Backspace and typing repeat; Enter, Esc
        // and Delete do not.
        for vk in [0x26, 0x28, 0x25, 0x27, 0x21, 0x08, 0x41] {
            assert!(repeats_with(vk, &[], true), "{vk:#x}");
        }
        for vk in [0x0d, 0x1b, 0x2e] {
            assert!(!repeats_with(vk, &[], true), "{vk:#x}");
        }
        // Outside a panel, a key blitz took that is no shortcut, as Enter
        // restarting an exited pane, does not go on to the new one.
        assert!(!repeats_with(0x0d, &[], false));

        let drops = |vk: u16, held: &[usize], taken: bool| {
            let mut t = String::new();
            let again = lp(0, false, true, 1) | 1 << 30;
            let k = msg_to_key(vk, again, &state(held, &[]), layout(US), &mut t);
            drops_repeat(&k, &[], taken, false)
        };
        // A held shortcut that asks to be pressed again is dropped even once
        // blitz lost track of its press, as when focus left and came back
        // while it was held.
        for (vk, held) in [(0x56, &[0xa2][..]), (0x57, CS), (0x55, CS), (0x2d, &[0xa1])] {
            assert!(drops(vk, held, false), "{vk:#x}");
        }
        // Closing a tab asks too, on whichever key it is bound to.
        let close_tab = [binding("ctrl+shift+q=close_tab").unwrap()];
        let mut t = String::new();
        let again = lp(0, false, true, 1) | 1 << 30;
        let k = msg_to_key(0x51, again, &state(CS, &[]), layout(US), &mut t);
        assert!(drops_repeat(&k, &close_tab, false, false), "close_tab");
        // Copy with nothing selected passes its press on, so the program
        // gets every ^C of a held Ctrl+C; taken, it copies once.
        for (vk, held) in [(0x43, &[0xa2][..]), (0x2d, &[0xa2])] {
            assert!(!drops(vk, held, false), "{vk:#x}");
            assert!(drops(vk, held, true), "{vk:#x} taken");
        }
        assert!(
            drops(0x0d, &[], true),
            "Enter that restarted an exited pane"
        );
        assert!(drops(0x1b, &[], true), "Esc that closed one");
        // Typing, and moving that repeats, go on.
        assert!(!drops(0x41, &[], false));
        assert!(!drops(0x0d, &[], false));
        assert!(!drops(0x09, &[0xa2], true), "CycleTab");
    }

    /// Enter and Space type the same on every layout.
    #[cfg(windows)]
    #[test]
    fn keymap_system_layout() {
        let mut t = String::new();
        let shift = state(&[0xa0], &[]);
        let k = msg_to_key(
            0x0d,
            lp(0x1c, false, true, 1),
            &shift,
            system_layout,
            &mut t,
        );
        assert_eq!((k.key, k.uc, k.text), (Key::Enter, 13, ""));
        let k = msg_to_key(
            0x20,
            lp(0x39, false, true, 1),
            &shift,
            system_layout,
            &mut t,
        );
        assert_eq!((k.key, k.uc, k.text), (Key::Char(' '), 32, " "));
    }

    // Windows' and Linux's chords.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keymap_find_and_prompt_jumps() {
        const LCTRL: usize = 0xa2;
        const LSHIFT: usize = 0xa0;
        assert_eq!(press(0x46, &[LCTRL, LSHIFT]), Some(Action::Find));
        assert_eq!(press(0x46, &[LCTRL]), None, "Ctrl+F goes to the program");
        assert_eq!(press(0x26, &[LCTRL, LSHIFT]), Some(Action::JumpToPrompt(1)));
        assert_eq!(
            press(0x28, &[LCTRL, LSHIFT]),
            Some(Action::JumpToPrompt(-1))
        );
        assert_eq!(press(0x26, &[LCTRL]), None);
    }

    /// Alt+Space opens the window menu, as everywhere else on Windows,
    /// unless a binding gives it back to the program.
    #[cfg(windows)]
    #[test]
    fn keymap_alt_space_opens_the_window_menu() {
        assert_eq!(press(0x20, &[0xa4]), Some(Action::SystemMenu));
        assert_eq!(press(0x20, &[0xa5]), Some(Action::SystemMenu), "right Alt");
        assert_eq!(press(0x20, &[]), None);
        assert_eq!(press(0x20, &[0xa2, 0xa4]), None, "AltGr+Space");
        let user: Vec<Binding> = binding("alt+space=none").into_iter().collect();
        let mut t = String::new();
        let k = msg_to_key(
            0x20,
            lp(0x39, false, true, 1),
            &state(&[0xa4], &[]),
            layout(US),
            &mut t,
        );
        assert_eq!(action(&k, &user), None);
    }
    #[test]
    fn keymap_update_and_zoom_keys_never_reach_the_program() {
        // Ctrl+Shift+U and Ctrl+Shift+Z would arrive as ^U and ^Z.
        for a in [Action::Update, Action::Zoom] {
            assert!(!passes_on(a), "{a:?}");
        }
        // Copy with nothing selected is Ctrl+C for the program, and a
        // divider that cannot move gives Alt+Shift+Arrows back.
        for a in [Action::Copy, Action::Paste, Action::Resize(Dir::Left)] {
            assert!(passes_on(a), "{a:?}");
        }
    }
}

#[cfg(all(test, not(windows)))]
mod winit_key_tests {
    use winit::keyboard::KeyCode;

    use super::*;

    fn press(code: KeyCode, base: &str, text: Option<&str>) -> WinitKey {
        WinitKey {
            code: Some(code),
            base: Some(base.into()).filter(|b: &String| !b.is_empty()),
            named: None,
            text: text.map(Into::into),
            down: true,
            dead: false,
        }
    }

    fn mods(f: impl FnOnce(&mut Mods)) -> Mods {
        let mut m = Mods::default();
        f(&mut m);
        m
    }

    #[test]
    fn letters_follow_the_layout_and_digits_their_place() {
        let mut text = String::new();
        // The A key of AZERTY is where US has Q.
        let k = winit_key(
            &press(KeyCode::KeyQ, "a", Some("a")),
            Mods::default(),
            &mut text,
        );
        assert_eq!((k.vk, k.key, k.text), (0x41, Key::Char('a'), "a"));
        assert_eq!((k.scan, k.us_base), (0x10, Some('q')));
        // AZERTY's 1 key types & and is still the 1 key.
        let k = winit_key(
            &press(KeyCode::Digit1, "&", Some("&")),
            Mods::default(),
            &mut text,
        );
        assert_eq!((k.vk, k.key), (0x31, Key::Char('&')));
        // A Russian layout's letters keep their US places.
        let k = winit_key(
            &press(KeyCode::KeyF, "а", Some("а")),
            Mods::default(),
            &mut text,
        );
        assert_eq!(k.vk, 0x46);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn option_types_its_own_character_and_is_meta_otherwise() {
        let mut text = String::new();
        let opt = mods(|m| m.lalt = true);
        // Option+2 types ™ on a US Mac.
        let k = winit_key(&press(KeyCode::Digit2, "2", Some("™")), opt, &mut text);
        assert!(!k.mods.lalt);
        assert_eq!((k.text, k.uc), ("™", 0x2122));
        // Where it changes nothing, it is Alt, as Meta for the shell.
        let k = winit_key(&press(KeyCode::KeyB, "b", Some("b")), opt, &mut text);
        assert!(k.mods.lalt);
        assert_eq!(k.text, "b");
        let left = WinitKey {
            named: Some(winit::keyboard::NamedKey::ArrowLeft),
            text: None,
            ..press(KeyCode::ArrowLeft, "", None)
        };
        let k = winit_key(&left, opt, &mut text);
        assert!(k.mods.lalt && k.extended);
        assert_eq!((k.vk, k.key, k.scan), (0x25, Key::Left, 0x4b));
    }

    #[test]
    fn ctrl_letters_carry_their_control_code() {
        let mut text = String::new();
        let ctrl = mods(|m| m.lctrl = true);
        let k = winit_key(&press(KeyCode::KeyC, "c", Some("c")), ctrl, &mut text);
        assert_eq!((k.vk, k.uc, k.text), (0x43, 3, "c"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn alt_is_meta_whatever_shift_types() {
        let mut text = String::new();
        let both = mods(|m| (m.lalt, m.lshift) = (true, true));
        let k = winit_key(&press(KeyCode::Digit1, "1", Some("!")), both, &mut text);
        assert!(k.mods.lalt);
        let mut out = Vec::new();
        vt::encode_key(&k, &Default::default(), &mut out);
        assert_eq!(out, b"\x1b!");
    }

    #[test]
    fn ctrl_gives_a_control_code_or_none() {
        let mut text = String::new();
        let ctrl_alt = mods(|m| (m.lctrl, m.lalt) = (true, true));
        let k = winit_key(
            &press(KeyCode::BracketLeft, "[", Some("[")),
            ctrl_alt,
            &mut text,
        );
        assert_eq!(k.uc, 0x1b);
        let mut out = Vec::new();
        vt::encode_key(&k, &Default::default(), &mut out);
        assert_eq!(out, b"\x1b\x1b");
        let k = winit_key(&press(KeyCode::Digit1, "1", Some("1")), ctrl_alt, &mut text);
        assert_eq!(k.uc, 0);
    }

    #[test]
    fn a_keypad_key_that_types_has_num_lock_on() {
        let mut text = String::new();
        let k = winit_key(
            &press(KeyCode::NumpadAdd, "+", Some("+")),
            Mods::default(),
            &mut text,
        );
        assert!(k.locks.num);
        let pam = vt::InputModes {
            deckpam: true,
            ..Default::default()
        };
        let mut out = Vec::new();
        vt::encode_key(&k, &pam, &mut out);
        assert_eq!(out, b"+");
        let k = winit_key(
            &press(KeyCode::KeyA, "a", Some("a")),
            Mods::default(),
            &mut text,
        );
        assert!(!k.locks.num);
    }

    #[test]
    fn caps_lock_shows_in_the_case_typed() {
        let mut text = String::new();
        let k = winit_key(
            &press(KeyCode::KeyA, "a", Some("A")),
            Mods::default(),
            &mut text,
        );
        assert!(k.locks.caps);
        let shift = mods(|m| m.lshift = true);
        let k = winit_key(&press(KeyCode::KeyA, "a", Some("A")), shift, &mut text);
        assert!(!k.locks.caps);
    }

    #[test]
    fn a_dead_key_types_nothing() {
        let mut text = String::new();
        let dead = WinitKey {
            dead: true,
            ..press(KeyCode::KeyE, "e", Some("´"))
        };
        let k = winit_key(&dead, mods(|m| m.lalt = true), &mut text);
        assert_eq!((k.key, k.text), (Key::Other, ""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_shortcuts_take_command() {
        let mut text = String::new();
        let cmd = mods(|m| m.lsuper = true);
        let k = winit_key(&press(KeyCode::KeyC, "c", Some("c")), cmd, &mut text);
        assert_eq!(action(&k, &[]), Some(Action::Copy));
        let k = winit_key(&press(KeyCode::Digit3, "3", Some("3")), cmd, &mut text);
        assert_eq!(action(&k, &[]), Some(Action::GoToTab(2)));
        let k = winit_key(&press(KeyCode::Slash, "/", Some("/")), cmd, &mut text);
        assert_eq!(action(&k, &[]), Some(Action::Keys));
        assert_eq!(keys_for(Action::Keys, &[]).as_deref(), Some("Cmd+/"));
        // Ctrl+C is the program's.
        let ctrl = mods(|m| m.lctrl = true);
        let k = winit_key(&press(KeyCode::KeyC, "c", Some("c")), ctrl, &mut text);
        assert_eq!(action(&k, &[]), None);
        assert_eq!(
            keys_for(Action::SplitDown, &[]).as_deref(),
            Some("Shift+Cmd+D")
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_keeps_the_windows_shortcuts() {
        let mut text = String::new();
        let both = mods(|m| (m.lctrl, m.lshift) = (true, true));
        let k = winit_key(&press(KeyCode::KeyT, "t", Some("T")), both, &mut text);
        assert_eq!(action(&k, &[]), Some(Action::NewTab));
        let k = winit_key(&press(KeyCode::Slash, "/", Some("?")), both, &mut text);
        assert_eq!(action(&k, &[]), Some(Action::Keys));
        let sup = mods(|m| m.lsuper = true);
        let k = winit_key(&press(KeyCode::KeyC, "c", Some("c")), sup, &mut text);
        assert_eq!(action(&k, &[]), None);
    }
}
