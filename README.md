# blitz

A fast terminal for running several Claude Code sessions, or any other
shells, side by side, and seeing at a glance which one is waiting for you.

> **Early alpha. Windows only for now.** Expect rough edges and missing
> features.

- Tabs and splits; every pane is its own shell.
- With two or more sessions, a sidebar lists them with their directory,
  git branch and state: *needs you*, *working*, *done*, *error*.
  Ctrl+Shift+B collapses it to a narrow rail of status dots.
- Claude Code hooks tell blitz when a session needs input or has
  finished, and the taskbar button flashes if blitz is in the background.
- Progress a program reports (OSC 9;4) shows in the sidebar and on the
  taskbar button.
- Themes: Ctrl+Shift+K previews them live; bring your own in Ghostty's
  format.
- A settings panel on Ctrl+, for the font, shell, sessions and more.
- A command palette on Ctrl+Shift+P lists every action with its keys,
  and `config.toml` can bind any of them to other keys.
- Selections that reach into scrollback, and Ctrl+click on links and
  file paths.
- Direct3D 11 and DirectWrite rendering, with a software fallback.

## Install

Windows 10 1809 or later, x64. From
[Releases](https://github.com/thomasbunch/blitz/releases), run the
`-setup.exe` (installs for the current user, no admin rights needed) or
unzip the `.zip` into a folder only you can change, such as one in your
user profile. Avoid a new folder directly under a drive like `C:\`: every
local user can write to it and could replace blitz's files. Builds are
not code-signed yet: if SmartScreen says "Windows protected your PC",
choose **More info**, then **Run anyway**.

The shell is PowerShell 7 if installed, else Windows PowerShell, else cmd.

The installer adds **Open in blitz** to the right-click menu of folders
and drives; on Windows 11 it is under **Show more options**.
Shift+right-click also offers **Open in new blitz window**. Untick the
option during setup to leave the menu alone.

blitz asks GitHub for the latest release when it starts and every six
hours. If there is a newer one, a strip under the panes says so. Ctrl+Shift+U
downloads the installer, checks it against the release's `SHA256SUMS.txt`
and restarts blitz on the new version. That ends every session, so blitz
asks you to press it twice if one is busy. A copy run from the zip opens
the release page instead. If the installer fails, blitz starts again on
the old version and the strip names the installer's log. With no strip,
Ctrl+Shift+U asks GitHub right away and says what it found.

## Build

Needs [Rust](https://rustup.rs); `rust-toolchain.toml` pins the toolchain.

```
cargo build --release
```

This produces `target\release\blitz.exe` and `blitz-hook.exe`; keep them
together. Releases also ship Microsoft's `conpty.dll` and `OpenConsole.exe`
(NuGet `Microsoft.Windows.Console.ConPTY` 1.25 or later, from
`runtimes\win-x64` and `build\native\runtimes\x64`). Put them next to
`blitz.exe` or set `BLITZ_CONPTY_DIR` to the full path of their folder.
Without them blitz uses the slower console host built into Windows, which
drops some features such as synchronized output.

## Claude Code

```
blitz setup claude
```

prints the hook settings to add to `~/.claude/settings.json`; blitz never
edits that file itself. The hooks run `blitz-hook.exe`, which does
nothing when Claude Code runs outside blitz. Remove them before you move
or uninstall blitz: Claude Code keeps running whatever is at that path.

## Sessions

Closing the window keeps its tabs, splits and folders, and blitz opens
them again the next time it starts; so does an update. A pane that was
running Claude Code reopens the conversation with `claude --resume`
once its shell is ready (this needs the hooks above). Typing `exit` in
the last pane ends it all, and the next start is fresh. **Open in blitz**
adds a tab to the running window, or to the reopened one; **Open in new
blitz window** opens a separate window that is never saved.

Saved state lives in `%LOCALAPPDATA%\blitz`.

## Settings

Ctrl+, opens the settings panel. Up and down choose a setting, left and
right change it, and Enter flips a switch or moves to the next value; a
click does the same. Typing searches the names and descriptions, Del puts
a setting back to its default, and Esc closes the panel. The font, its
size and the theme change at once; the panel says when the others take
effect. A dot marks each setting changed from its default.

The panel saves to `%APPDATA%\blitz\config.toml`, which you can also edit
yourself: one `key = value` per line, `#` starts a comment. blitz reads
the file again whenever it is saved, and keeps your comments when it
writes it.

| Key | Default | |
|---|---|---|
| `theme` | `"light:blitz light,dark:blitz dark"` | A theme name, or a light and a dark one to follow the Windows app mode. See [Themes](#themes) |
| `font_family` | `"Cascadia Mono"` | Falls back to Cascadia Mono, then Consolas, when not installed |
| `font_size` | `11` | In points, 4 to 72 |
| `shell` | `""` | Path of the program new panes run; empty picks PowerShell 7, then Windows PowerShell, then cmd |
| `shell_integration` | `true` | Let PowerShell and cmd report their folder and prompts to blitz |
| `scrollback_lines` | `10000` | Lines of history each new pane keeps, up to 100000 |
| `restore_session` | `true` | Reopen the last window's tabs, splits and folders |
| `restore_claude` | `true` | Resume the Claude Code sessions they were running |
| `restore_scrollback` | `false` | Save each pane's last 1000 lines when blitz closes and show them again above the new prompt. Off by default because old output can contain secrets |
| `check_updates` | `true` | Look for a newer release |
| `flash` | `true` | Flash the taskbar button when a session needs you |
| `bell_attention` | `true` | Treat a bell in a background pane as needing you |
| `scenery` | `"off"` | Pixel scenery behind the panes: `"off"`, `"stars"`, `"hills"` or `"snow"` |
| `mascot` | `false` | Show the spark, a critter at the foot of the sidebar that follows your sessions |
| `keybind` | | Binds a key to an action; one line per key. See [Key bindings](#key-bindings) |

blitz run, the last row of the panel, is a game rather than a setting:
Enter starts it, and it keeps its best score in
`%LOCALAPPDATA%\blitz\run-best`.

## Themes

Ctrl+Shift+K opens the theme picker. The arrow keys show each theme on
the whole window as you go, typing narrows the list, Enter keeps the
theme (it writes `theme = "..."` to `config.toml`) and Esc puts the old
one back. With a light and a dark theme set, Enter changes the one for
the mode Windows is in now.

Built in: **blitz** dark and light (the default), **blitz ember** and
**blitz tide**, each dark and light, and Catppuccin Mocha and Latte,
GitHub Dark and Light, Gruvbox Dark and Light, Rose Pine and Rose Pine
Dawn, Solarized Dark and Light, and Tokyo Night and Tokyo Night Day.

To make your own, put a file in `%APPDATA%\blitz\themes`; its file name,
without a `.conf` ending, is the theme's name. The format is Ghostty's,
so the files from
[iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes/tree/master/ghostty)
work as they are. Edits show up while blitz runs.

```
# %APPDATA%\blitz\themes\Midnight
background = #10131a
foreground = #d8dee9
cursor-color = #eceff4
selection-background = #2e3440
palette = 0=#1d2129
palette = 1=#e06c75
# ... through palette = 15
accent = #f2b84b
```

`palette` sets colours 0 to 15. Colours a file leaves out come from the
blitz theme of the same lightness. The sidebar and pane headers are mixed
from the background and foreground; to choose them yourself, add any of
`accent` (marks sessions that need you), `sidebar-background`, `border`,
`rule`, `row-focus`, `title`, `dim`, `message`, `track`, `progress`,
`error`, `header-background`, `header-line`, `header-title`,
`header-cwd`, `rail-focus`, `rail-work`, `idle`, `label`, `label-focus`
and `top-track`. The
[blitz dark](crates/blitz/themes/blitz-dark) file sets every key and is a
good place to start.

## Selection and links

| Mouse | Action |
|---|---|
| Drag | Select; past the top or bottom of the pane it scrolls |
| Double-click, triple-click | Select a word, or the whole line as the program printed it |
| Alt+drag | Select a block of columns |
| Shift+click | Extend the selection |
| Ctrl+click | Open a link |
| Wheel | Scroll; in less, man and other full-screen programs, press the arrow keys |

A selection can reach into scrollback and stays on its text while
output scrolls; it ends when output rewrites the text. A double click
takes a path or URL whole, and dragging after a double or triple click
selects by words or lines.

Holding Ctrl underlines the link under the pointer: a hyperlink a
program printed, a web address, or the path of a file that exists,
relative ones from the pane's folder. Only web, mail and local file
links open. A file that would run when opened, such as a program,
script, shortcut or installer, is shown selected in Explorer instead.

When a program takes the mouse itself, as Claude Code does in
fullscreen, clicks go to it. Hold Shift as well to use blitz's
selection: Shift+drag selects, and Ctrl+Shift+click opens a link.

## Find and prompts

Ctrl+Shift+F opens a find bar at the top right of the focused pane. It
searches the pane's scrollback and screen as you type, ignoring case
unless what you type has a capital letter, and tints every match in
view. Enter or F3 goes to the next match up, Shift+Enter or Shift+F3 to
the next one down, and Esc closes the bar, leaving the view where it is.
A full-screen program is searched on its screen only.

With shell integration on, Ctrl+Shift+Up and Ctrl+Shift+Down scroll to
the previous and next prompt; down from the last one goes back to the
bottom. With no prompt that way, or in a full-screen program, the keys
go to the program.

## Default keys

| Keys | Action |
|---|---|
| Ctrl+Shift+T | New tab |
| Ctrl+Shift+W | Close pane (the last pane closes its tab) |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous tab |
| Ctrl+1 to Ctrl+9 | Go to tab |
| Ctrl+Shift+R | Split right |
| Ctrl+Shift+D | Split down |
| Ctrl+Alt+Arrows | Move focus between panes |
| Alt+Shift+Arrows | Resize the focused pane (or drag the line between panes) |
| Ctrl+Alt+Shift+Arrows | Swap the focused pane with its neighbour |
| Ctrl+Shift+Z | Zoom the focused pane to fill the tab, or show every pane again |
| Ctrl+Shift+J | Jump to the next session that needs you |
| Ctrl+Shift+B | Expand or collapse the sidebar |
| Ctrl+Shift+U | Update, or look for a newer release now |
| Ctrl+Shift+K | Pick a theme |
| Ctrl+, | Settings |
| Ctrl+Shift+P | Command palette |
| Ctrl+=, Ctrl+-, Ctrl+0 | Font size up, down, and back to the setting, until blitz restarts |
| F11 | Full screen |
| Ctrl+C, Ctrl+Shift+C, Ctrl+Insert | Copy, when text is selected |
| Ctrl+V, Ctrl+Shift+V, Shift+Insert | Paste |
| Shift+PgUp, Shift+PgDn | Scroll |
| Ctrl+Shift+F | Find in the scrollback |
| Ctrl+Shift+Up, Ctrl+Shift+Down | Previous, next prompt |

A zoomed pane stays zoomed until you zoom again, move focus to another
pane, split or resize.

The command palette lists every action with its keys. Typing narrows
the list, the arrow keys choose, and Enter or a click runs the action;
Esc closes it. Giving the panes equal space has no keys by default, so
it is only in the palette.

## Key bindings

`keybind` lines in `config.toml` change the keys. Each binds a chord to
an action, or with `none` gives a chord back to the program:

```
keybind = ctrl+shift+e=split_right
keybind = ctrl+shift+r=none
keybind = alt+f11=command_palette
```

A chord is any of `ctrl`, `shift` and `alt` and one key, joined by `+`:
a letter, a digit, `f1` to `f24`, `left`, `right`, `up`, `down`, `home`,
`end`, `pageup`, `pagedown`, `insert`, `delete`, `tab`, `enter`, `esc`,
`space`, `backspace`, or a punctuation key by its character or its name,
such as `,` or `comma` and `=` or `plus`. The modifiers must match
exactly. A binding replaces the default on the same chord, and the other
defaults stay; Ctrl+1 to Ctrl+9 go to a tab unless a binding takes them.
Lines blitz cannot read are skipped.

The actions are `copy`, `paste`, `scroll_page_up`, `scroll_page_down`,
`new_tab`, `close_pane`, `next_tab`, `previous_tab`, `split_right`,
`split_down`, `focus_left`, `focus_right`, `focus_up`, `focus_down`,
`jump_to_attention`, `toggle_sidebar`, `update`, `theme_picker`,
`settings`, `zoom`, `resize_left`, `resize_right`, `resize_up`,
`resize_down`, `swap_left`, `swap_right`, `swap_up`, `swap_down`,
`equalize`, `font_size_up`, `font_size_down`, `font_size_reset`,
`fullscreen`, `command_palette`, `find`, `previous_prompt` and
`next_prompt`. Typing a name in the command palette finds its action.

## License

[MIT](LICENSE)
