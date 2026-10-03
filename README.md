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
- Themes: Ctrl+Shift+K previews them live; bring your own in Ghostty's
  format.
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

blitz asks GitHub for the latest release when it starts and once a day.
If there is a newer one, a strip under the panes says so. Ctrl+Shift+U
downloads the installer, checks it against the release's `SHA256SUMS.txt`
and restarts blitz on the new version. That ends every session, so blitz
asks you to press it twice if one is busy. A copy run from the zip opens
the release page instead.

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

Optional, in `%APPDATA%\blitz\config.toml`, one `key = value` per line.
The theme follows the file as soon as it is saved; the rest is read when
blitz starts.

| Key | Default | |
|---|---|---|
| `theme` | `"light:blitz light,dark:blitz dark"` | A theme name, or a light and a dark one to follow the Windows app mode. See [Themes](#themes) |
| `restore_session` | `true` | Reopen the last window's tabs, splits and folders |
| `restore_claude` | `true` | Resume the Claude Code sessions they were running |
| `restore_scrollback` | `false` | Save each pane's last 1000 lines when blitz closes and show them again above the new prompt. Off by default because old output can contain secrets |
| `check_updates` | `true` | Look for a newer release |
| `flash` | `true` | Flash the taskbar button when a session needs you |
| `bell_attention` | `true` | Treat a bell in a background pane as needing you |

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
| Ctrl+Shift+J | Jump to the next session that needs you |
| Ctrl+Shift+B | Expand or collapse the sidebar |
| Ctrl+Shift+U | Update, when a newer release is available |
| Ctrl+Shift+K | Pick a theme |
| Ctrl+C, Ctrl+Shift+C, Ctrl+Insert | Copy, when text is selected |
| Ctrl+V, Ctrl+Shift+V, Shift+Insert | Paste |
| Shift+PgUp, Shift+PgDn | Scroll |

## License

[MIT](LICENSE)
