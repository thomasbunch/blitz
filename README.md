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
- Direct3D 11 and DirectWrite rendering, with a software fallback.

## Install

Windows 10 1809 or later, x64. From
[Releases](https://github.com/thomasbunch/blitz/releases), run the
`-setup.exe` (installs for the current user, no admin rights needed) or
unzip the `.zip` anywhere. Builds are not code-signed yet: if SmartScreen
says "Windows protected your PC", choose **More info**, then **Run anyway**.

The shell is PowerShell 7 if installed, else Windows PowerShell, else cmd.

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
nothing when Claude Code runs outside blitz.

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
| Ctrl+C, Ctrl+Shift+C, Ctrl+Insert | Copy, when text is selected |
| Ctrl+V, Ctrl+Shift+V, Shift+Insert | Paste |
| Shift+PgUp, Shift+PgDn | Scroll |

## License

[MIT](LICENSE)
