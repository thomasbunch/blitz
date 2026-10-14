# Changelog

What changes in each blitz release. The newest release comes first. A
release's section here becomes its notes on GitHub, so write it for the
people who use blitz.

## [0.0.5] - Unreleased

The biggest release yet: a command palette, your own key bindings, find,
notifications, and Claude Code marks that need no setup. Windows only.

### Keys and the palette

- **Ctrl+Shift+P** opens a command palette with every action and its keys.
- `keybind = ctrl+shift+e=split_right` lines in `config.toml` bind any chord
  to any action, or to text to type, and `=none` gives a chord back to the
  program.
- Resize panes with **Alt+Shift+arrows** or by dragging the line between
  them, swap them with **Ctrl+Alt+Shift+arrows**, and zoom one with
  **Ctrl+Shift+Z**. A double click on a divider gives the panes equal space.
- **Ctrl+=**, **Ctrl+-** and **Ctrl+0** change the font size, **F11** goes
  full screen, and **Ctrl+Shift+O** goes to a session by name.

### Find, selection and links

- **Ctrl+Shift+F** finds text in the screen and the scrollback.
- **Ctrl+Shift+Up** and **Down** jump between prompts.
- Selections reach into the scrollback, by word, line or block, and each pane
  keeps its own.
- **Ctrl+click** opens links and files, at their line in your editor with
  `editor_uri`. It never runs a program.
- **Ctrl+Shift+Space** labels the links, paths and hashes in view, to copy
  or open one from the keyboard.

### Claude Code and other agents

- Claude Code in a pane loads blitz's hooks by itself. `blitz setup claude`
  is only needed for older Claude Code or managed settings.
- Looking at a question no longer answers it. It stays *needs you* until you
  type or click in its pane.
- The sidebar shows how long a turn ran and how long a question waited, and
  names sessions after their task. You can rename sessions and tabs.
- Closing a busy pane, tab or window asks first. **Reopen the last closed
  pane** in the palette brings it back with its conversation.
- Other agents can report too, with `blitz-hook notify <state> [message]`.
- Files and screenshots paste into Claude Code.

### Telling you from the background

- Windows notifications while blitz is in the background, set by `toasts`. A
  click goes to the session.
- The taskbar button carries the colour of the session that wants you, and
  progress a program reports.
- `sound`, `global_jump` (Ctrl+Alt+J from any program) and `keep_awake`
  settings, all off by default.

### Window and look

- A second launch brings blitz to the front instead of opening the same
  sessions twice. The window comes back where it was.
- Text too close to its background is moved just far enough to read, and an
  error reads apart from *needs you* without colour.
- Underline kinds, strikethrough, overline, program cursor colours and bar
  cursors are drawn. `line_height` spaces the lines.
- **Easter eggs** in the settings panel: pixel scenery, the spark and the
  blitz run game. All off by default.

### Shells and settings

- A tab on any installed shell from the palette, and `shell` takes a command
  line such as `wsl.exe -d Ubuntu`.
- `env = NAME=VALUE` lines set variables for new panes.
- `blitz setup shell bash` or `zsh` prints prompt marks for WSL and Git Bash.
- A notice names the `config.toml` lines blitz skipped.

### Updates

- Updates go through the proxy Windows is set to use.
- A failed update starts the old blitz again and says why on the next start.
- **Ctrl+Shift+U** with no update offered looks for one right away.

### Fixed

- Claude Code *needs you* and *done* marks never arrived since 0.0.3: titles
  were cut short.
- A shell that failed to start froze blitz on every start.
- Holding a key answered its own "press again" question, so a held
  Ctrl+Shift+W closed busy panes.
- A bare shell name could run a program from the current folder.
- A crash in the terminal parser takes down only its pane, and a crash on
  the window's thread is written to `%LOCALAPPDATA%\blitz\crash.txt`.
- Many more in the terminal core, input and rendering. See
  [#21](https://github.com/thomasbunch/blitz/pull/21).

### Known gaps

- No macOS or Linux builds yet.
- Shell output is saved only when blitz exits normally, not at sign-out.
- No screen reader support in the panes.

## [0.0.4] - 2026-10-03

- **Ctrl+,** opens a settings panel. Font, size and theme change at once,
  and the panel writes to `config.toml` with your comments kept.
- `config.toml` takes `font_family`, `font_size`, `shell`,
  `shell_integration` and `scrollback_lines`.
- `bell_attention` works: a bell in a pane you are not looking at needs you.
- blitz has its own icon. On Windows 11 the title bar takes the theme's
  colours, and programs hear when the theme changes (mode 2031).
- Typing while output streams is quicker, colourful output parses about 1.7
  times as fast, and the first frame comes about 25 ms sooner.

## [0.0.3] - 2026-10-03

- **Open in blitz** on the right-click menu of folders and drives opens a tab
  in the running blitz.
- blitz reopens your tabs, splits, folders and Claude Code conversations, and
  with `restore_scrollback` the old output too.
- **Ctrl+Shift+K** picks from 18 built-in themes, or Ghostty theme files in
  `%APPDATA%\blitz\themes`. A light and a dark theme can follow Windows.
- `config.toml`, the first config file. Edits apply while blitz runs.
- Text rewraps when a pane's width changes.

## [0.0.2] - 2026-10-02

- blitz updates itself from GitHub releases with **Ctrl+Shift+U**, checked
  against `SHA256SUMS.txt`.
- Everything a pane prints is treated as untrusted: output cannot fake a
  session's state, copy leaves out hidden text, multi-line pastes ask first,
  and floods of queries are capped.

## [0.0.1] - 2026-10-02

- The first build: tabs and splits, a session sidebar with *needs you* and
  *done* marks for Claude Code, Direct3D 11 and DirectWrite rendering, and
  Microsoft's ConPTY bundled for low input latency.

[0.0.5]: https://github.com/thomasbunch/blitz/compare/v0.0.4...main
[0.0.4]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.4
[0.0.3]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.3
[0.0.2]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.2
[0.0.1]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.1
