# Changelog

What changes in each blitz release. The newest release comes first. A
release's section here becomes its notes on GitHub, so write it for the
people who use blitz.

## [Unreleased]

### Keys

- **Ctrl+Shift+/** (**Cmd+/** on macOS) lists every action and the keys
  that run it, grouped by what they do, with a dot on the ones you
  changed. The command palette and the settings panel open it too, and
  typing narrows it by name or by keys.
- Enter on an action waits for new keys and adds them, saying first what
  they run now. A key alone (but F1 to F24), or a typing key with only
  Shift, stays the program's. Del puts an action's default keys back.
  Changes apply at once and are saved as `keybind` lines in
  `config.toml`, keeping the rest of the file.

### Files

- **Ctrl+Shift+E** (**Shift+Cmd+E** on macOS) shows the files in the
  pane's folder over the pane, with the highlighted file beside them.
  Typing narrows the list, Enter opens a folder, Backspace goes up, Tab
  moves to the file and Esc closes it. The program underneath keeps
  running.
- Ctrl+click on a text file's path, as in Claude Code's
  `Update(src/app.rs)`, opens it there at its line. Turn
  `open_files_in_blitz` off, or set an editor in `editor_uri`, to open it
  elsewhere as before.

### Sidebar

- Double-clicking a session's row collapses the sidebar to the rail, and
  double-clicking one on the rail expands it again.
- Panes a program opened with `blitz pane` are listed under its session,
  indented, in their own tab's group.

### Themes

- Fifteen new built-in themes. Radiant: **blitz neon** and **blitz
  volt**. Retro: **blitz phosphor** and **blitz phosphor amber**. Plain:
  **blitz paper light**, and **blitz contrast** dark and light, whose
  text meets WCAG AAA. And Dracula, Nord, One Dark, One Light, Night
  Owl, Kanagawa Wave, Ayu Light and Everforest Light.
- **blitz ember** and **blitz tide** no longer look like blitz dark and
  light with a tint: ember is orange on dark brown or peach, tide aqua on
  deep sea or pale surf.
- GitHub Dark and Light are gone, as blitz dark and light are near
  copies of them. A `theme` that names one gets blitz dark or light in
  its place. To keep the original, save `GitHub Dark Default` or
  `GitHub Light Default` from
  [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes/tree/master/ghostty)
  in the themes folder as `GitHub Dark` or `GitHub Light`.
- Each theme's swatch in the picker shows its text colour.

### Usage

- **Claude Code usage and cost** in the command palette shows what your
  Claude Code sessions on this PC would cost at API prices: this week by
  day, model and project, what agents spent, the tokens by kind and what
  the prompt cache saved, last week's total and the 5-hour window under
  way. Turn `usage_tracking` on in the settings first; it is off by
  default.
- The week follows your plan's weekly reset, set with `usage_reset_day`
  and `usage_reset_hour`, and the expanded sidebar shows the week's
  figure at its foot.

### Scripting panes

- `blitz pane split`, `tab`, `send`, `read`, `list` and `close` let a
  script or an agent in a pane open panes of its own, type into them and
  read what they show. A program reaches only the panes it opened, never
  types into one waiting for you, and its panes are not saved with the
  window while it is open.

## [0.0.5] - 2026-10-15

The biggest release yet: a command palette, your own key bindings, find,
notifications, Claude Code marks that need no setup, and the first builds
for macOS. Linux builds are experimental.

### macOS and Linux

- blitz runs on macOS 11 or later, as a universal `blitz.app` for Apple
  silicon and Intel. It is not notarized yet, so macOS stops the first
  start: choose **Open Anyway** in System Settings, Privacy & Security, or
  run `xattr -d com.apple.quarantine /Applications/blitz.app`.
- An experimental Linux x64 build. It passes CI on Ubuntu but has had no
  other testing, so please report what breaks.
- Shortcuts on a Mac use Cmd, as Terminal does: **Cmd+T** opens a tab,
  **Cmd+D** splits, **Cmd+W** closes, **Cmd+F** finds and **Cmd+click**
  opens links. Option types its own characters, and is Meta where it types
  none. Key bindings take `cmd` (or `super`) and `opt`.
- Drawing goes through Metal on macOS and Vulkan or OpenGL on Linux.
- Notifications through Notification Center or `notify-send`, a badge and a
  bounce on the Dock icon, and `keep_awake` keeps the Mac from sleeping.
- Settings live in `~/.config/blitz`. blitz says when there is a new release,
  and the notice opens its page; it updates itself only on Windows.

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

- On macOS and Linux: no `global_jump`, no progress on the Dock icon, no
  window menu on Alt+Space, and glyphs come from swash rather than Core Text.
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

[0.0.5]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.5
[0.0.4]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.4
[0.0.3]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.3
[0.0.2]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.2
[0.0.1]: https://github.com/thomasbunch/blitz/releases/tag/v0.0.1
