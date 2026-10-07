# blitz

A fast terminal for running several Claude Code sessions, or any other
shells, side by side, and seeing at a glance which one is waiting for you.

> **Early alpha. Windows only for now.** Expect rough edges and missing
> features.

- Tabs and splits; every pane is its own shell.
- With two or more sessions, a sidebar lists them with their directory,
  git branch and state: *needs you*, *working*, *done*, *error*.
  Ctrl+Shift+B collapses it to a narrow rail of status dots, and a click
  on the rail expands it again; a window under 800 pixels wide (at 100%
  scaling) shows the rail. Click a session or a tab's heading to go
  there. Sessions that do not fit are counted at the foot, in the accent
  colour when one of them needs you, and a click there goes to it.
- Claude Code's title tells blitz when a session works and when it
  stops, and its hooks, which blitz sets up itself, when it needs input.
  The window title starts with how many need you, as in `(2) pwsh`. If
  blitz is in the background, a Windows notification says so and takes
  you there when clicked, and the taskbar button flashes and shows the
  session's dot.
- In PowerShell with PSReadLine (its default), a command that runs for 10
  seconds or more and ends in a pane you are not looking at marks it
  *done*, or *error* with its exit code.
- Progress a program reports (OSC 9;4) shows in the sidebar and on the
  taskbar button.
- Themes: Ctrl+Shift+K previews them live; bring your own in Ghostty's
  format.
- A settings panel on Ctrl+, for the font, shell, sessions and more.
- A command palette on Ctrl+Shift+P lists every action with its keys,
  and `config.toml` can bind any of them to other keys.
- Selections that reach into scrollback, and Ctrl+click on links and
  file paths.
- Files dropped on a pane paste as their paths, and Ctrl+V pastes a
  screenshot into Claude Code.
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
option during setup to leave the menu alone. Setup for the current user
can also start blitz when you sign in, with the tabs and sessions you
left; that option is off unless you tick it.

From a shell, `blitz DIR` opens a tab in that folder, and `blitz .` in
the one you are in, in the blitz already running if there is one;
`--new-window` gives it a window of its own. `blitz --help` lists the
rest.

blitz asks GitHub for the latest release when it starts and every six
hours, through the proxy Windows is set to use. If there is a newer
one, the foot of the sidebar says so, or a strip under the panes while
the sidebar is collapsed or not shown: a click on it opens the release
notes, and its × hides it until a newer release. Ctrl+Shift+U downloads
the installer, checks it against the release's `SHA256SUMS.txt` and
restarts blitz on the new version. That ends every session and closes
any other blitz window, so blitz asks first. When a session is busy in
the main blitz window and no other is open, the first press leaves the
update to install when you close blitz, and the next press restarts
now. A copy run from the
zip opens the release page instead. If the installer fails, blitz starts
again on the old version and a strip under the panes names the
installer's log. With nothing shown, Ctrl+Shift+U asks GitHub right away
and says what it found. The settings panel, under **Check for updates**,
says why the last look or update failed.

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

Claude Code 2.1.280 or later reports to blitz with nothing to set up.
blitz keeps a small Claude Code plugin in
`%LOCALAPPDATA%\blitz\claude-plugin-…`, one for each copy of blitz, whose
hooks run the `blitz-hook.exe` next to `blitz.exe`, and every pane
loads it through `CLAUDE_CODE_PLUGIN_DIRS`. blitz never edits Claude
Code's settings, and writes no plugin where other users could replace
`blitz-hook.exe`. If Claude Code works for a while and no hook reports,
blitz says so once.

An older Claude Code, or managed settings that turn off plugin folders,
needs the hooks in `~/.claude/settings.json`. **Claude Code setup** in
the command palette copies them; merge their `"hooks"` into that file.
The installer puts blitz in `%LOCALAPPDATA%\Programs\blitz` (in
`Program Files` when installed for all users), and from PowerShell

```
& "$env:LOCALAPPDATA\Programs\blitz\blitz.exe" setup claude | Out-Host
```

prints them too, and points out hooks the file already runs, which with
a newer Claude Code report every event twice. Remove pasted hooks before
you move or uninstall blitz: Claude Code keeps running whatever is at
that path, and the uninstaller reminds you. For Claude Code inside WSL,
`setup claude --wsl` prints the hooks for `~/.claude/settings.json` in
WSL; panes pass their token into WSL, so the hooks find their pane.

Without the hooks blitz still sees from Claude Code's title when a
session starts working and when it stops, and with `bell_attention` on
a bell or a notification (OSC 9 or 777) in a pane you are not looking
at marks it as needing you until you look. Claude Code rings one when
it waits for you if its settings have
`"preferredNotifChannel": "terminal_bell"`. The hooks add what the title
cannot show: a question waiting for you, the prompt and the reply under
the session's name, the conversation to resume, and that the pane runs
Claude Code at all, which names the session after its task. Only they,
and a session that ends in an error, raise a Windows notification, and
only they keep the PC awake: any program can ring a bell or set a
title, so a bell alone flashes the taskbar button and no more.

### Session marks

| Mark | State | Set when | Cleared when |
|---|---|---|---|
| Dot in the accent colour, *needs you* | needs you | Claude Code asks for permission or a decision, or has a plan ready; a bell rings in a pane you are not looking at | you answer a question by typing, pasting or clicking in its pane (looking only outlines it), or the next hook says what it does now; a bell's mark clears when you look |
| Thin bar under the session, *working · 4m* | working | a prompt is sent, or the title shows Claude Code working | the turn ends |
| Hollow ring, *done* | done | a turn ends while you look elsewhere | you look at it |
| Red square, *error* | error | Claude Code stops on an error, or the program exits with a failure code | you look at it; a program that exited stays until its pane closes |
| None | idle | nothing is running, or Claude Code has ended | |

You look at a session when its pane has focus and blitz is the window in
front; half a minute without a key or mouse touch counts as away, even
with blitz in front. Ctrl+Shift+J goes to questions you have not seen
first, then those you have, then finished sessions, then failed ones.

### Other agents

Agents with hooks of their own can report the same way: `blitz-hook
notify <state> [message]`, where the state is `working`, `needs-you`,
`done`, `error` or `idle`, writes to the pane's console, past the output
the agent reads, and does nothing outside blitz. For blitz installed for
the current user, with your user name in the path:

Codex, in `~/.codex/config.toml`; Codex adds its last reply as the message:

```toml
notify = ['C:\Users\you\AppData\Local\Programs\blitz\blitz-hook.exe', 'notify', 'done']
```

Gemini CLI, in `~/.gemini/settings.json`:

```json
{
  "hooks": {
    "BeforeAgent": [{ "hooks": [{ "type": "command", "command": "C:\\Users\\you\\AppData\\Local\\Programs\\blitz\\blitz-hook.exe notify working" }] }],
    "AfterAgent": [{ "hooks": [{ "type": "command", "command": "C:\\Users\\you\\AppData\\Local\\Programs\\blitz\\blitz-hook.exe notify done" }] }],
    "Notification": [{ "hooks": [{ "type": "command", "command": "C:\\Users\\you\\AppData\\Local\\Programs\\blitz\\blitz-hook.exe notify needs-you" }] }]
  }
}
```

## Sessions

A session running Claude Code goes by the task its title names, which
Claude's `/rename` changes; others go by their program, with a number
when two share a name. A tab goes by the folder of its focused pane.
**Rename session** and **Rename tab** in the command palette name them
yourself; an empty name goes back to the automatic one.

Closing the window keeps its tabs, splits and folders, and blitz opens
them again the next time it starts; so does an update, of blitz or of
Windows. Names you gave and results you had not seen yet come back too.
A pane that was running Claude Code reopens the conversation with
`claude --resume` once its shell is ready (this needs the hooks above).
Typing `exit` in the last pane ends it all, and the next start is fresh.
Starting blitz while it runs brings its window to the front, on the
virtual desktop you are on. **Open in blitz** adds a tab to the running
window, or to the reopened one; **Open in new blitz window** opens a
separate window that is never saved. So does blitz run as
administrator, which says so in its title.

Closing a pane, a tab or the window ends what runs in it, so blitz asks
first while a session there is busy: Claude Code working or waiting for
you, or a shell command running. Do the same again to close it; any other
key leaves it open and does what it always does. **Reopen the last closed
pane** in the command palette brings it back beside the focused pane,
resuming its Claude Code conversation. A program that exits with an error
leaves its pane open with the exit code: Enter starts it again in place,
Esc closes the pane.

Saved state lives in `%LOCALAPPDATA%\blitz`. If blitz ever stops after
an internal error, it writes what happened to `crash.txt` there, and the
next start says so once and keeps the file as `last-crash.txt`.

## Shells

Besides the shell in the settings, the command palette opens a tab on
any shell installed: PowerShell, cmd, Git Bash or a WSL distribution.
The pane keeps its shell when blitz reopens it. New panes get the PATH
blitz started with, in its order, then the folders Windows' Path has
gained since, so a program installed while blitz runs is found without
restarting blitz.

With shell integration on, PowerShell and cmd mark each prompt and tell
blitz their folder, around the prompt you have, also one that
oh-my-posh or posh-git sets later. For bash in Git Bash or WSL,

```
blitz setup shell bash
```

prints lines to add to the end of `~/.bashrc` (`zsh` prints them for
`~/.zshrc`). They do nothing outside blitz. blitz follows only Windows folders, so in
WSL only folders under `/mnt` are reported, and the lines need
`BLITZ_PANE_TOKEN`, which reaches WSL only when `WSLENV` lists it.

## Settings

Ctrl+, opens the settings panel. Up and down choose a setting, left and
right change it, and Enter flips a switch or moves to the next value; a
click does the same. Typing searches the names and descriptions, Del puts
a setting back to its default, and Esc closes the panel. The font, its
size and the theme change at once; the panel says when the others take
effect. A dot marks each setting changed from its default.

The panel saves to `%APPDATA%\blitz\config.toml`, which you can also edit
yourself: one `key = value` per line, `#` after a space starts a
comment. **Open config.toml** in the command palette opens it, in
Notepad when no program opens `.toml` files. blitz reads the file again whenever it is saved, and
keeps your comments when it writes it. A line it cannot use is skipped,
and a dim notice in the pane says which. A theme or font that is not
there shows in the panel with what blitz uses instead.

| Key | Default | |
|---|---|---|
| `theme` | `"light:blitz light,dark:blitz dark"` | A theme name, or a light and a dark one to follow the Windows app mode. See [Themes](#themes) |
| `font_family` | `"Cascadia Mono"` | Falls back to Cascadia Mono, then Consolas, when not installed. Icons the font lacks come from an installed Nerd Font |
| `font_size` | `11` | In points, 4 to 72 |
| `line_height` | `1` | Space between the lines in the panes, 0.8 to 2 times the font's own |
| `shell` | `""` | The program new panes run, with any arguments, such as `wsl.exe -d Ubuntu`; empty picks PowerShell 7, then Windows PowerShell, then cmd |
| `shell_integration` | `true` | Let PowerShell and cmd report their folder and prompts to blitz |
| `scrollback_lines` | `10000` | Lines of history each new pane keeps, up to 100000 |
| `restore_session` | `true` | Reopen the last window's tabs, splits and folders |
| `restore_claude` | `true` | Resume the Claude Code sessions they were running |
| `restore_scrollback` | `false` | Save each pane's last 1000 lines when blitz closes and show them again above the new prompt. Off by default because old output can contain secrets |
| `keep_awake` | `false` | Keep the PC from going to sleep by itself while Claude Code works in a session, as its hooks say |
| `check_updates` | `true` | Look for a newer release |
| `flash` | `true` | Flash the taskbar button when a session needs you |
| `toasts` | `"needs-you"` | Windows notifications while blitz is in the background: `"needs-you"` for sessions that need you or failed, `"all"` for finished ones too, or `"off"`. Clicking one takes you to the session |
| `sound` | `false` | Play a sound when a session wants you while blitz is in the background: the notification's, or the default beep without one |
| `global_jump` | `false` | Let Ctrl+Alt+J bring blitz to the front from any program, on the session that needs you |
| `bell_attention` | `true` | Treat a bell or a notification in a background pane as needing you |
| `right_click_paste` | `true` | A right click copies the selection, or pastes when nothing is selected |
| `editor_uri` | `""` | Ctrl+click on a file path opens it at its line through this URI, such as `"vscode://file/{path}:{line}:{col}"`. Empty opens the file with its program, or shows it in Explorer when it has none |
| `scenery` | `"off"` | Pixel scenery behind the panes: `"off"`, `"stars"`, `"hills"` or `"snow"` |
| `mascot` | `false` | Show the spark, a critter at the foot of the sidebar that follows your sessions |
| `keybind` | | Binds a key to an action; one line per key. See [Key bindings](#key-bindings) |
| `env` | | Sets a variable in new panes, as in `env = RUST_LOG=debug`; one line per variable |

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

To make your own, put a file in `%APPDATA%\blitz\themes`, which **Open
themes folder** in the command palette opens; its file name, without a
`.conf` ending, is the theme's name. The format is Ghostty's,
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

`palette` sets colours 0 to 15, and `selection-foreground` and
`cursor-text` the colour of selected text and of text on the cursor.
Colours a file leaves out come from the blitz theme of the same
lightness. The sidebar and pane headers are mixed from the background
and foreground; to choose them yourself, add any of `accent` (marks
sessions that need you; its dots are made darker or lighter until they
stand out), `sidebar-background`, `border`,
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
| Right-click | Copy the selection, or paste when nothing is selected |
| Wheel | Scroll the pane under the pointer, by as many lines as Windows is set to |

A selection can reach into scrollback and stays on its text while
output scrolls, or when the pane's width changes; it ends when output
rewrites the text. Each pane keeps its own selection while another has
focus, and the view holds still while you drag. A double click takes a
path or URL whole, without the full stop after it, and dragging after
a double or triple click selects by words or lines. The command palette
can also select all of a pane's text, or the last command's output
between two of blitz's prompts.

Ctrl+Shift+Space labels each URL, file path and commit hash in view
with a letter. Type the letter to copy what it marks, or Shift and the
letter to open it; any other key puts the labels away.

Ctrl+C copies the selection while it is in view; scrolled out of view,
Ctrl+C goes to the program as usual. Ctrl+Shift+C and Ctrl+Insert copy
it either way, and with nothing selected Ctrl+Shift+C never reaches the
program as Ctrl+C. When output rewrites a selection, the next copy
still takes the text as it was, and says so, though plain Ctrl+C goes
to the program, as nothing shows; anything else you do drops it. A dim
line says how many lines were copied; if another program is holding
the clipboard, the selection stays so you can copy again. **Copy
without indent** in the command palette leaves out the marks Claude
Code puts before replies and tool output, and the indent the lines
share.

Holding Ctrl underlines the link under the pointer: a hyperlink a
program printed, a web address, or the path of a file that exists,
relative ones from the pane's folder and `~` from your user folder. A
path counts in Claude Code's `Update(src/app.rs)`, in a Markdown link,
and as a bare `name.ext` when that file is in the pane's folder. A path
with spaces is a link only as a hyperlink, and a path through a
symbolic link or junction is none. Only web and local file links open.
Folders, text, source code, images and PDFs open with their program;
any other file, such as a program, script, shortcut or installer, is
shown selected in Explorer instead, and so is one no program opens.

With an editor set in `editor_uri`, Ctrl+click opens a file there at
the line and column after its path, as in `src/app.rs:12:5` or
`src/app.ts(12,5)`. This works in Claude Code's fullscreen too, where
other clicks go to Claude Code.

When a program takes the mouse itself, as Claude Code does in
fullscreen, clicks and the wheel go to it, and the pointer is an arrow
rather than an I-beam. Hold Shift as well to use blitz's: Shift+drag
selects, Ctrl+Shift+click opens a link, and Shift+right-click copies or
pastes. Over a full-screen program, such as less or man, the wheel
presses the arrow keys unless the program turns that off, but never in
Claude Code, where they would bring back an earlier prompt.

## Paste and drop

Ctrl+V pastes text. One line copied with its line break comes without
it, so it is not run. More lines first show what they are and wait for
a second Ctrl+V, until you confirm one while the program takes pastes
as such (bracketed paste); so does a paste over 5 KiB into a program
that does not. Claude Code never asks once its hooks have reported.
Nothing is pasted into a pane whose program has exited.

Files copied in Explorer paste as their paths, in quotes when they hold
a space or anything else a shell would read, and so do files dropped on
a pane. As either shell may run inside the other, a name no quoting
keeps safe in both cmd and PowerShell waits for a second Ctrl+V. A Git
Bash pane gets single quotes, which keep any name, and never asks about
one. A folder dropped on the sidebar opens in a new tab. Windows does
not let you drop from Explorer onto blitz running as administrator.
With an image on the clipboard and no text, such as a screenshot,
Ctrl+V in a Claude Code pane whose hooks have reported sends Alt+V,
which pastes the image; other programs get Ctrl+V.

In the find bar, the command palette, the theme picker and the settings
panel, a paste adds the first line of the clipboard and Ctrl+Backspace
deletes a word.

Programs can copy to the clipboard with OSC 52, as tmux and Neovim do
over SSH, and the pane says so. Only the pane you are in can, while
blitz is in front, and no program can read the clipboard.

## Find and prompts

Ctrl+Shift+F opens a find bar at the top right of the focused pane,
holding the selected text or else what you last looked for, ready to
type over. It searches the pane's scrollback and screen as you type,
ignoring case unless what you type has a capital letter, and shows every
match in view in the selection colour, the current one outlined. Enter
or F3 goes to the next match up, Shift+Enter or Shift+F3 to the next one
down, and Esc closes the bar, leaving the view where it is and the match
selected, so Ctrl+C copies it. Esc before you type or move puts the view
back and selects nothing. Any other shortcut does what it does and
closes the bar, unless it scrolls, and a click in the pane closes it
too. A full-screen program is searched on its screen only, as the bar
says.

With shell integration on, Ctrl+Shift+Up and Ctrl+Shift+Down scroll to
the previous and next prompt; down from the last one goes back to the
bottom. With no prompt that way, or in a full-screen program, the keys
go to the program.

A pane scrolled back says how many lines are below the view; a click on
that, or typing, goes back to the bottom.

## Default keys

| Keys | Action |
|---|---|
| Ctrl+Shift+T | New tab |
| Ctrl+Shift+W | Close pane (the last pane closes its tab) |
| Ctrl+Tab, Ctrl+Shift+Tab | Next, previous tab |
| Ctrl+Shift+PgUp, Ctrl+Shift+PgDn | Move the tab left, right |
| Ctrl+1 to Ctrl+8, Ctrl+9 | Go to that tab, to the last tab |
| Ctrl+Shift+R | Split right |
| Ctrl+Shift+D | Split down |
| Ctrl+Alt+Arrows | Move focus between panes |
| Alt+Shift+Arrows | Resize the focused pane (or drag the line between panes) |
| Ctrl+Alt+Shift+Arrows | Swap the focused pane with its neighbour |
| Ctrl+Shift+Z | Zoom the focused pane to fill the tab, or show every pane again |
| Ctrl+Shift+J | Jump to the next session that needs you, or back when none does |
| Ctrl+Shift+O | Go to a session, picked from a list you can type to narrow |
| Ctrl+Shift+B | Expand or collapse the sidebar |
| Ctrl+Shift+U | Update, or look for a newer release now |
| Ctrl+Shift+K | Pick a theme |
| Ctrl+, | Settings |
| Ctrl+Shift+P | Command palette |
| Ctrl+=, Ctrl+-, Ctrl+0 | Font size up, down, and back to the setting, until blitz restarts |
| Ctrl+Shift+=, Ctrl+NumpadAdd, Ctrl+NumpadSubtract, Ctrl+Numpad0 | The same, with + typed with Shift and on the keypad |
| Ctrl+wheel | Font size up or down, unless the program takes the mouse |
| F11 | Full screen |
| Alt+Space | Window menu: move, size, minimize, close |
| Ctrl+C, Ctrl+Shift+C, Ctrl+Insert | Copy the selection; Ctrl+C only while it is in view |
| Ctrl+V, Ctrl+Shift+V, Shift+Insert | Paste |
| Shift+PgUp, Shift+PgDn | Scroll |
| Ctrl+Shift+Home, Ctrl+Shift+End | Scroll to the top, to the bottom |
| Ctrl+Shift+F | Find in the scrollback |
| Ctrl+Shift+Space | Quick select a link, path or hash to copy or open |
| Ctrl+Shift+Up, Ctrl+Shift+Down | Previous, next prompt |

A zoomed pane stays zoomed until you zoom again, move focus to another
pane, split or resize. With `global_jump` on, Ctrl+Alt+J jumps from any
program: it brings blitz to the front on the session that needs you.

The command palette lists every action with its keys, and a **New tab**
for each shell installed. Typing narrows the list, the arrow keys
choose, and Enter or a click runs the action; Esc closes it. When no
action matches, Enter looks for what you typed in the settings instead.
Starting a new Claude Code session in a split, giving the panes equal
space, closing a whole tab, reopening the last closed pane, moving a
pane to a new tab, renaming a session or a tab, copying without indent,
selecting all, selecting the last command's output, clearing the
scrollback, resetting the terminal, opening config.toml or the themes
folder and Claude Code setup have no keys by default, so they are only
in the palette; renaming takes the name on the palette's line.
Double-clicking the line between panes also gives them equal space.
Clearing the scrollback also clears the screen above the line the cursor
is on, which moves to the top, so nothing cleared comes back when the
pane is resized. Resetting the terminal turns off what a program that
crashed can leave on, such as mouse reports, a hidden cursor or keys
sent as escape codes; the text stays.
**Report an issue**, also only in the palette, starts a GitHub issue
that says which blitz, Windows, renderer and ConPTY you run, but no file
or folder names.

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
such as `,` or `comma` and `=` or `plus`, and on the keypad `numpad0` to
`numpad9`, `numpadadd`, `numpadsubtract`, `numpadmultiply`,
`numpaddivide` and `numpaddecimal`. Case and spaces do not matter. The
modifiers must match exactly. A binding replaces the default on the
same chord, and the other defaults stay. Ctrl+1 to Ctrl+9 go to a tab
unless a binding takes them, on layouts where those keys type digits;
on others, such as French AZERTY, they go to the program unless bound
to `go_to_tab_1` to `go_to_tab_8` or `last_tab`.
Lines blitz cannot read are skipped.

The actions are `copy`, `copy_without_indent`, `paste`, `select_all`,
`select_last_output`, `scroll_page_up`, `scroll_page_down`,
`scroll_to_top`, `scroll_to_bottom`, `clear_scrollback`,
`reset_terminal`, `new_tab`, `close_pane`, `close_tab`, `reopen_closed`,
`rename_session`, `rename_tab`, `next_tab`, `previous_tab`,
`move_tab_left`, `move_tab_right`, `move_pane_to_new_tab`,
`split_right`, `split_down`, `new_claude`, `focus_left`, `focus_right`,
`focus_up`, `focus_down`, `jump_to_attention`, `go_to_session`,
`toggle_sidebar`, `update`, `theme_picker`, `settings`, `open_config`,
`open_themes`, `zoom`, `resize_left`, `resize_right`, `resize_up`,
`resize_down`, `swap_left`, `swap_right`, `swap_up`, `swap_down`,
`equalize`, `font_size_up`, `font_size_down`, `font_size_reset`,
`fullscreen`, `command_palette`, `find`, `quick_select`,
`previous_prompt`, `next_prompt`, `system_menu`, `claude_setup`,
`report_issue`, `go_to_tab_1` to `go_to_tab_8` and `last_tab`. Typing a
name in the command palette finds its action.

A binding can also type text into the focused pane: `text:` and the
text, with `\e` for Esc, `\r` for Enter, `\n`, `\t`, `\s` for a space,
`\\` for a backslash and `\xNN` for any byte. Put the line in single
quotes to keep a `#` in the text.

```
keybind = ctrl+shift+g=text:git status\r
keybind = 'alt+c=text:claude --continue # last session\r'
```

A key with nothing to do goes to the program in the pane: Ctrl+C with no
text selected, or a resize or swap with no split or neighbour that way.
Held down, the keys that move focus, switch or move tabs, scroll, resize
and swap repeat; the others act once, so a held key never answers its own
"press again".

## Accessibility

- The cursor does not blink, and nothing is animated unless you turn on
  `scenery` or `mascot`; those keep still while Windows' animation
  effects are off. `flash = false` stops the taskbar button flashing.
- With a Windows contrast theme on and the `theme` setting at its
  default, blitz takes the contrast theme's colours.
- blitz keeps the system caret, hidden, on the cursor, so Magnifier and
  other tools that follow the text cursor follow it. Claude Code draws a
  cursor of its own instead; set `CLAUDE_CODE_ACCESSIBILITY=1` in your
  environment to have it use the real one.
- Sessions that need you are marked in the theme's `accent` colour and
  errors in its `error` colour. If the two are hard to tell apart, set
  them in a theme file (see [Themes](#themes)). Claude Code's `/theme`
  has colour-blind-friendly themes for its own output.
- Ctrl+= and Ctrl+- change the font size; `font_size` sets it.
- Screen readers cannot read the panes yet.

## License

[MIT](LICENSE)
