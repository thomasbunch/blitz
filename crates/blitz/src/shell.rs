//! Default shell detection and shell integration (OSC 133 and OSC 7).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Finds the shell to start when none is configured: `pwsh.exe` on PATH,
/// then the newest `%ProgramFiles%\PowerShell\<n>\pwsh.exe`, then Windows
/// PowerShell, then `%ComSpec%`. Looked up for each pane, so one installed
/// while blitz runs is found.
pub fn detect() -> PathBuf {
    detect_with(pane_var)
}

/// A variable as a new pane gets it: PATH from [`fresh_path`], the rest as
/// blitz has them.
pub fn pane_var(k: &str) -> Option<OsString> {
    let var = std::env::var_os(k);
    if !k.eq_ignore_ascii_case("PATH") {
        return var;
    }
    let inherited = var.map(|v| v.to_string_lossy().into_owned());
    fresh_path(registry_path(true), registry_path(false), inherited).map(Into::into)
}

/// PATH for a new pane: the system's Path, then the user's, as the registry
/// holds them now, so a program installed while blitz runs is found; then
/// the folders only `inherited`, blitz's own PATH, has, such as those of the
/// developer prompt blitz was started from. Just `inherited` when the
/// registry has neither.
pub fn fresh_path(
    machine: Option<String>,
    user: Option<String>,
    inherited: Option<String>,
) -> Option<String> {
    if machine.is_none() && user.is_none() {
        return inherited;
    }
    let same = |a: &str, b: &str| {
        let trim = |s: &str| s.trim_end_matches(['\\', '/']).to_lowercase();
        trim(a) == trim(b)
    };
    let mut out: Vec<&str> = Vec::new();
    for dir in [&machine, &user, &inherited]
        .into_iter()
        .flatten()
        .flat_map(|p| p.split(';'))
    {
        if !dir.is_empty() && !out.iter().any(|o| same(o, dir)) {
            out.push(dir);
        }
    }
    Some(out.join(";"))
}

/// The system's (`machine`) or the user's Path in the registry, with the
/// variables in it expanded.
#[cfg(windows)]
fn registry_path(machine: bool) -> Option<String> {
    use windows::Win32::Foundation::ERROR_MORE_DATA;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::w;

    let (key, sub) = if machine {
        let sub = w!(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment");
        (HKEY_LOCAL_MACHINE, sub)
    } else {
        (HKEY_CURRENT_USER, w!("Environment"))
    };
    let mut buf = vec![0u16; 2048];
    // The value can grow between the size query and the read.
    for _ in 0..4 {
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `size` bytes; both outlive the call. A
        // REG_EXPAND_SZ value comes back expanded.
        let r = unsafe {
            RegGetValueW(
                key,
                sub,
                w!("Path"),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if r == ERROR_MORE_DATA {
            buf.resize((size as usize).div_ceil(2), 0);
            continue;
        }
        r.ok().ok()?;
        let units = &buf[..(size as usize / 2).min(buf.len())];
        return Some(
            String::from_utf16_lossy(units)
                .trim_end_matches('\0')
                .into(),
        );
    }
    None
}

#[cfg(not(windows))]
fn registry_path(_machine: bool) -> Option<String> {
    None
}

/// [`detect`] with the environment supplied by the caller.
pub fn detect_with(var: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    let root = system_root(&var);
    if let Some(exe) = pwsh(&var) {
        return exe;
    }
    let ps = windows_powershell(&root);
    if ps.is_file() {
        return ps;
    }
    var("ComSpec").map_or_else(|| root.join("System32").join("cmd.exe"), PathBuf::from)
}

pub(crate) fn system_root(var: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    PathBuf::from(var("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
}

fn windows_powershell(root: &Path) -> PathBuf {
    root.join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
}

/// PowerShell 7: `pwsh.exe` on PATH, else the newest
/// `%ProgramFiles%\PowerShell\<n>\pwsh.exe`.
fn pwsh(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // Walk PATH ourselves: SearchPathW would also look in the current
    // directory, and spawning where.exe would flash a console window.
    if let Some(path) = var("PATH") {
        for dir in std::env::split_paths(&path) {
            // An empty or relative entry is the current directory again,
            // and a bare result would be looked up there by CreateProcessW.
            if !dir.is_absolute() {
                continue;
            }
            let exe = dir.join("pwsh.exe");
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    let pf = var("ProgramFiles")?;
    std::fs::read_dir(Path::new(&pf).join("PowerShell"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let digits = name.split(|c: char| !c.is_ascii_digit()).next()?;
            let major: u32 = digits.parse().ok()?;
            let exe = e.path().join("pwsh.exe");
            // `7` beats `7-preview` whichever the folder lists first.
            exe.is_file().then_some(((major, digits == name), exe))
        })
        .max_by_key(|(key, _)| *key)
        .map(|(_, exe)| exe)
}

/// The shells the settings panel offers, as (name, path): first the
/// automatic choice, whose path is empty, then each one installed.
pub fn choices() -> Vec<(String, String)> {
    let found = installed_with(pane_var);
    let auto = detect();
    let name = (found.iter())
        .find(|(_, p)| Path::new(p) == auto)
        .map_or_else(|| auto.to_string_lossy().into_owned(), |(n, _)| n.clone());
    let mut out = vec![(format!("Automatic ({name})"), String::new())];
    out.extend(found);
    out
}

/// Shells found with the environment supplied by the caller, as (name,
/// path).
fn installed_with(var: impl Fn(&str) -> Option<OsString>) -> Vec<(String, String)> {
    let root = system_root(&var);
    let sys = root.join("System32");
    let git = var("ProgramFiles").map(|pf| Path::new(&pf).join("Git").join("bin").join("bash.exe"));
    [
        ("PowerShell 7", pwsh(&var)),
        ("Windows PowerShell", Some(windows_powershell(&root))),
        ("Command Prompt", Some(sys.join("cmd.exe"))),
        ("WSL", Some(sys.join("wsl.exe"))),
        ("Git Bash", git),
    ]
    .into_iter()
    .filter_map(|(name, exe)| {
        let exe = exe.filter(|e| e.is_file())?;
        Some((name.to_string(), exe.to_string_lossy().into_owned()))
    })
    .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `pwsh.exe` or Windows PowerShell's `powershell.exe`.
    PowerShell,
    Cmd,
    Other,
}

pub fn kind(program: &Path) -> Kind {
    // The program is a Windows path. Take the file name by hand so `\` and
    // a drive prefix separate it on every host, not only on Windows.
    let path = program.to_string_lossy();
    let name = path.rsplit(['\\', '/', ':']).next().unwrap_or_default();
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase());
    match stem.as_deref() {
        Some("pwsh" | "powershell") => Kind::PowerShell,
        Some("cmd") => Kind::Cmd,
        _ => Kind::Other,
    }
}

/// Quotes one argument the way `CommandLineToArgvW` and the MSVC runtime
/// split it again.
pub fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// Wraps the user's prompt with OSC 133 marks and reports the directory with
/// OSC 7. Works on PowerShell 5.1 and 7, so it avoids `` `e ``. The A mark is
/// tagged `blitz=<token>` with the pane's `BLITZ_PANE_TOKEN` so the terminal
/// can tell it apart from marks that other programs print. Each prompt first
/// leaves an alternate screen a program left on, then sends a soft reset
/// (DECSTR) and puts back default tab stops, so the console host drops
/// margins, insert mode and charsets a program left behind, as the terminal
/// does at the mark. It leaves the screen with `?1049h` then `?1049l`:
/// conhost restores the saved cursor on `?1049l` even on the main screen, so
/// there the pair saves and restores the same cursor, and on the alternate
/// screen the console host and the terminal both restore the cursor saved
/// when the program switched. A command's end mark carries the code of the
/// program it ran, or 1 when it failed otherwise: `$LASTEXITCODE` keeps the
/// last program's code through later commands, so it counts when it changed
/// or no new error says a cmdlet failed. A user's strict mode would stop
/// the script reading what is not set yet, so it is off in its own scopes.
///
/// It goes on the command line as it is, so `Get-Process` or Task Manager
/// shows what blitz runs. It holds no double quote, which Windows
/// PowerShell and PowerShell 7 read differently in an argument.
///
/// A prompt defined later, as oh-my-posh or posh-git may, is wrapped again
/// when the next command is read. Each wrapper keeps the prompt it wraps,
/// and one called from inside another, as by a prompt that calls the one
/// it replaced, adds no marks of its own.
pub const POWERSHELL_INTEGRATION: &str = r"if (-not (Test-Path variable:global:__blitz)) {
  $global:__blitz = @{ Exec = $false; In = $false; Token = $env:BLITZ_PANE_TOKEN }
  $global:__blitz.Wrap = { param($orig) {
    $ok = $global:?; Set-StrictMode -Off; $c = $global:LASTEXITCODE
    $new = $global:Error.Count -and -not [object]::ReferenceEquals($global:Error[0], $global:__blitz.Err)
    $code = if ($ok) { 0 } elseif ($c -and ($c -ne $global:__blitz.Last -or -not $new)) { $c } else { 1 }
    if ($global:__blitz.In) { return & $orig }
    $e = [string][char]27; $b = [char]7; $s = '$e[?1049h$e[?1049l$e[!p$e[?5W'.Replace('$e', $e)
    if ($global:__blitz.Exec) { $s += $e + ']133;D;' + $code + $b; $global:__blitz.Exec = $false }
    $s += $e + ']133;A;blitz=' + $global:__blitz.Token + $b
    if ($PWD.Provider.Name -eq 'FileSystem') {
      $p = $PWD.ProviderPath -replace '^\\\\\?\\UNC\\', '\\' -replace '^\\\\\?\\', ''
      try { $s += $e + ']7;' + [Uri]::new($p).AbsoluteUri + $b } catch {}
    }
    $global:__blitz.In = $true
    if (-not $ok) { Write-Error 'x' -ErrorAction Ignore }
    try { $s + (& $orig) + $e + ']133;B' + $b } finally { $global:__blitz.In = $false }
  }.GetNewClosure() }
  $function:global:prompt = & $global:__blitz.Wrap $function:prompt
  if (Get-Module PSReadLine) {
    $global:__blitz.RL = $function:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
      $l = & $global:__blitz.RL; Set-StrictMode -Off; $global:__blitz.Exec = $true
      $global:__blitz.Last = $global:LASTEXITCODE; $global:__blitz.Err = if ($global:Error.Count) { $global:Error[0] }
      if ($function:prompt -and [string]$function:prompt -notlike '*__blitz*') {
        $function:global:prompt = & $global:__blitz.Wrap $function:prompt
      }
      [Console]::Write([string][char]27 + ']133;C' + [char]7); $l
    }
  }
}";

/// cmd's prompt `own`, such as `$P$G`, with the same marks and reset. cmd
/// cannot report exit codes, nor expand variables in its prompt, so the
/// token is written in.
pub fn cmd_prompt(token: &str, own: &str) -> String {
    format!(
        r"$e[?1049h$e[?1049l$e[!p$e[?5W$e]133;D$e\$e]133;A;blitz={token}$e\$e]9;9;$P$e\{own}$e]133;B$e\"
    )
}

/// Whether `prompt` is a [`cmd_prompt`], which blitz started from a blitz
/// pane inherits. It carries the other pane's token, so it is not the
/// user's own and is not passed on.
pub fn is_blitz_prompt(prompt: &std::ffi::OsStr) -> bool {
    prompt.to_string_lossy().contains("]133;A;blitz=")
}

/// The user's own part of `prompt`, cmd's PROMPT as blitz has it: all of
/// it, or the prompt a [`cmd_prompt`] wraps.
fn own_prompt(prompt: &str) -> &str {
    if !is_blitz_prompt(prompt.as_ref()) {
        return prompt;
    }
    (prompt.split_once(r"$e]9;9;$P$e\"))
        .and_then(|(_, own)| own.strip_suffix(r"$e]133;B$e\"))
        .unwrap_or("$P$G")
}

/// The program a command line starts and the arguments after it, split
/// as `CommandLineToArgvW` takes its first argument: a quoted one runs to
/// the next quote, any other to the first space or tab.
pub fn split_program(cmdline: &str) -> (&str, &str) {
    let s = cmdline.trim_start_matches([' ', '\t']);
    match s.strip_prefix('"') {
        Some(rest) => rest.split_once('"').unwrap_or((rest, "")),
        None => s.split_at(s.find([' ', '\t']).unwrap_or(s.len())),
    }
}

/// Whether PowerShell `args` already say what to run, which the
/// integration's own `-Command` would take the place of. PowerShell takes
/// any start of a parameter's name, and `-e` is `-EncodedCommand`.
fn runs_command(args: &str) -> bool {
    args.split_whitespace().any(|a| {
        let a = a.trim_start_matches(['-', '/']).to_ascii_lowercase();
        let names = ["command", "file", "encodedcommand", "commandwithargs"];
        !a.is_empty() && (names.iter().any(|n| n.starts_with(&a)) || a == "ec" || a == "cwa")
    })
}

/// A command line ready for `CreateProcessW`, plus variables to add to the
/// child's environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub cmdline: String,
    pub env: Vec<(String, String)>,
}

/// The program and arguments of a `shell` setting. A path to a program
/// that holds spaces needs no quotes, as the setting once took only a path.
fn parts(shell: &str) -> (&str, &str) {
    let shell = shell.trim();
    let whole = Path::new(shell);
    if whole.is_absolute() && whole.is_file() {
        (shell, "")
    } else {
        split_program(shell)
    }
}

/// A `shell` setting as the settings panel shows one it does not list:
/// the program's file name, then the arguments as typed.
pub fn label(shell: &str) -> String {
    let (program, args) = parts(shell);
    let name = program.rsplit(['\\', '/']).next().unwrap_or_default();
    format!("{name}{args}")
}

/// Builds the command line for `shell`, the `shell` setting: a program and
/// its arguments, or empty for [`detect`]. Shell integration is added only
/// when `integrate` is set; otherwise the command runs exactly as
/// configured. `token` is the pane's `BLITZ_PANE_TOKEN`.
pub fn launch(shell: &str, integrate: bool, token: &str) -> Launch {
    let (program, args) = match parts(shell) {
        ("", _) => (detect(), ""),
        (program, args) => (PathBuf::from(program), args),
    };
    let mut out = Launch {
        cmdline: quote(&program.to_string_lossy()) + args,
        env: Vec::new(),
    };
    if integrate {
        match kind(&program) {
            Kind::PowerShell if runs_command(args) => {}
            Kind::PowerShell => {
                // -Command still runs when the execution policy forbids
                // scripts, and the user's profile has already loaded by then.
                out.cmdline += " -NoLogo -NoExit -Command ";
                out.cmdline += &quote(POWERSHELL_INTEGRATION);
            }
            Kind::Cmd => {
                let own = std::env::var_os("PROMPT").map(|p| p.to_string_lossy().into_owned());
                let own = own.as_deref().map_or("$P$G", own_prompt);
                out.env.push(("PROMPT".into(), cmd_prompt(token, own)));
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_leave_the_alternate_screen() {
        let reset = "$e[?1049h$e[?1049l$e[!p$e[?5W";
        assert!(POWERSHELL_INTEGRATION.contains(&format!("$s = '{reset}'.Replace('$e', $e)")));
        assert!(cmd_prompt("1", "$P$G").starts_with(reset));
        let prompt = cmd_prompt("1", "$P$G").replace("$e", "\x1b");
        let mut t = vt::Terminal::new(vt::Options::default());
        // On the main screen the cursor stays where it is.
        t.feed(b"\x1b[3;5H");
        t.feed(prompt.as_bytes());
        assert_eq!(t.cursor().1, 2);
        // A program left on the alternate screen: back to the main screen
        // and the cursor saved when it switched.
        t.feed(b"\x1b[5;1H\x1b[?1049h\x1b[9;9H");
        t.feed(prompt.as_bytes());
        assert!(!t.input_modes().alt_screen);
        assert_eq!(t.cursor().1, 4);
    }

    #[test]
    fn quote_round_trips_msvc_rules() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote(r"C:\Program Files\x"), r#""C:\Program Files\x""#);
        assert_eq!(quote(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote(r"dir with\ trailing\"), r#""dir with\ trailing\\""#);
    }

    /// What the system itself makes of a quoted argument.
    #[test]
    #[cfg(windows)]
    fn quote_round_trips_through_the_system_parser() {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        for arg in [
            r"a\\b c",
            "\\\"",
            r"x\",
            "a\tb",
            "a\\\\\"b c",
            "",
            " ",
            "\\",
            r"trailing\\",
            r"C:\Program Files\x\",
            "\"\"",
            "a\"b\"c",
            "\u{e9}t\u{e9} \u{2713}",
        ] {
            let line = windows::core::HSTRING::from(format!("p {}", quote(arg)));
            let mut n = 0;
            // SAFETY: a valid string and out pointer; the array is freed below.
            let got = unsafe {
                let argv = CommandLineToArgvW(&line, &mut n);
                assert!(!argv.is_null());
                let got = (n == 2).then(|| (*argv.add(1)).to_string().unwrap());
                let _ = LocalFree(Some(HLOCAL(argv.cast())));
                got
            };
            assert_eq!(got.as_deref(), Some(arg), "{arg:?} as {line}");
        }
    }

    #[test]
    fn only_blitz_prompts_are_its_own() {
        use std::ffi::OsStr;
        assert!(is_blitz_prompt(OsStr::new(&cmd_prompt("5eed", "$P$G"))));
        assert!(!is_blitz_prompt(OsStr::new("$P$G")));
        // Another terminal's marks are the user's business.
        assert!(!is_blitz_prompt(OsStr::new(r"$e]133;A$e\$P$G")));
    }

    #[test]
    fn cmd_keeps_the_users_own_prompt() {
        let mine = r"$T $e]9;9;$P$e\$P$_$+$G";
        let wrapped = cmd_prompt("5eed", mine);
        assert!(wrapped.ends_with(&format!(r"$P$e\{mine}$e]133;B$e\")));
        assert_eq!(own_prompt(mine), mine);
        // From a blitz pane: the prompt that pane's wraps.
        assert_eq!(own_prompt(&wrapped), mine);
        assert_eq!(own_prompt(&cmd_prompt("5eed", "$P$G")), "$P$G");
    }

    #[test]
    fn other_shells_run_as_configured() {
        let bash = r#""C:\Program Files\Git\bin\bash.exe" --login -i"#;
        let want = Launch {
            cmdline: bash.into(),
            env: Vec::new(),
        };
        assert_eq!(launch(bash, true, "t"), want);
        assert_eq!(launch("wsl.exe", true, "t").cmdline, "wsl.exe");
        assert_eq!(
            launch(" wsl.exe -d Ubuntu ", true, "t").cmdline,
            "wsl.exe -d Ubuntu"
        );
    }

    #[test]
    fn new_panes_get_the_path_the_registry_holds_now() {
        let s = |v: &str| Some(v.to_string());
        // Git installed since blitz started is in the registry only, and a
        // developer prompt's folder only in blitz's own PATH.
        assert_eq!(
            fresh_path(
                s(r"C:\Windows\system32;C:\Program Files\Git\cmd"),
                s(r"C:\Users\me\.cargo\bin;"),
                s(r"C:\VS\bin;C:\WINDOWS\System32\;;C:\Users\me\.cargo\bin"),
            ),
            s(r"C:\Windows\system32;C:\Program Files\Git\cmd;C:\Users\me\.cargo\bin;C:\VS\bin")
        );
        assert_eq!(fresh_path(None, None, s("a;b")), s("a;b"));
        assert_eq!(fresh_path(None, s("u"), None), s("u"));
        assert_eq!(fresh_path(None, None, None), None);
    }

    #[test]
    #[cfg(windows)]
    fn the_registry_path_comes_expanded() {
        let p = registry_path(true).expect("the system's Path");
        let p = p.to_lowercase();
        assert!(p.contains(r"\system32"), "{p}");
        assert!(!p.contains("%systemroot%"), "{p}");
        let fresh = pane_var("path").expect("a PATH");
        assert!(
            fresh
                .to_string_lossy()
                .to_lowercase()
                .contains(r"\system32")
        );
    }

    /// The setting once took only a path, written without quotes.
    #[test]
    fn a_path_with_spaces_is_one_program() {
        let dir = std::env::temp_dir().join(format!("blitz shell {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sh = dir.join("sh.exe");
        std::fs::write(&sh, b"").unwrap();
        let got = launch(&sh.to_string_lossy(), true, "t").cmdline;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, format!("\"{}\"", sh.display()));
    }

    #[test]
    fn powershell_arguments_stay_and_integration_follows_them() {
        let ps = |s: &str| launch(s, true, "t").cmdline;
        let integrated = ps("pwsh -NoProfile -ExecutionPolicy Bypass");
        assert!(
            integrated.starts_with("pwsh -NoProfile -ExecutionPolicy Bypass -NoLogo -NoExit "),
            "{integrated}"
        );
        assert!(ps("powershell.exe -ex Bypass").len() > 100);
        // Arguments that run something of their own keep the shell as set.
        for s in [
            "pwsh -NoProfile -Command Get-Date",
            "pwsh -c Get-Date",
            "powershell.exe /File x.ps1",
            "pwsh -f x.ps1",
            "pwsh -e ZQBjAGgAbwA=",
            "pwsh -ec ZQBjAGgAbwA=",
            "pwsh -cwa x",
            "pwsh --command x",
        ] {
            assert_eq!(ps(s), s);
        }
    }

    /// What the system itself takes as the program.
    #[test]
    #[cfg(windows)]
    fn split_program_agrees_with_the_system_parser() {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        for line in [
            "pwsh",
            "pwsh -NoLogo",
            "wsl.exe\t-d Ubuntu",
            r#""C:\Program Files\Git\bin\bash.exe" --login -i"#,
            r#""C:\Program Files\x.exe""#,
            r#""C:\a b\x.exe"-i"#,
            r#"C:\a\"b c" d"#,
            r#""unterminated x"#,
        ] {
            let mut n = 0;
            // SAFETY: a valid string and out pointer; the array is freed below.
            let argv0 = unsafe {
                let argv = CommandLineToArgvW(&windows::core::HSTRING::from(line), &mut n);
                assert!(!argv.is_null());
                let first = (*argv).to_string().unwrap();
                let _ = LocalFree(Some(HLOCAL(argv.cast())));
                first
            };
            assert_eq!(split_program(line).0, argv0, "{line}");
        }
        assert_eq!(split_program(r#""a b" -c"#), ("a b", " -c"));
        assert_eq!(split_program("x"), ("x", ""));
    }

    #[test]
    fn choices_start_with_the_automatic_one() {
        let c = choices();
        assert!(
            c[0].0.starts_with("Automatic (") && c[0].1.is_empty(),
            "{c:?}"
        );
        assert!(
            c[1..]
                .iter()
                .all(|(n, p)| !n.is_empty() && Path::new(p).is_file()),
            "{c:?}"
        );
    }

    #[test]
    fn kinds() {
        assert_eq!(kind(Path::new(r"C:\x\PowerShell.exe")), Kind::PowerShell);
        assert_eq!(kind(Path::new("pwsh")), Kind::PowerShell);
        assert_eq!(kind(Path::new("CMD.EXE")), Kind::Cmd);
        assert_eq!(kind(Path::new("bash.exe")), Kind::Other);
    }

    #[test]
    fn launch_integration() {
        let ps = launch(r#""C:\Program Files\PowerShell\7\pwsh.exe""#, true, "t");
        let script = format!("\"{POWERSHELL_INTEGRATION}\"");
        let head = ps
            .cmdline
            .strip_suffix(&script)
            .expect("the script, readable");
        assert_eq!(
            head,
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo -NoExit -Command "#
        );
        assert!(!POWERSHELL_INTEGRATION.contains('"'));
        assert_eq!(launch("pwsh.exe", false, "t").cmdline, "pwsh.exe");
    }

    /// The script reaches PowerShell as one argument, as written.
    #[test]
    #[cfg(windows)]
    fn the_powershell_script_is_one_argument() {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        let line = windows::core::HSTRING::from(launch("pwsh", true, "t").cmdline);
        let mut n = 0;
        // SAFETY: a valid string and out pointer; the array is freed below.
        let last = unsafe {
            let argv = CommandLineToArgvW(&line, &mut n);
            assert!(!argv.is_null());
            let last = (*argv.add(n as usize - 1)).to_string().unwrap();
            let _ = LocalFree(Some(HLOCAL(argv.cast())));
            last
        };
        assert_eq!((n, last.as_str()), (5, POWERSHELL_INTEGRATION));
    }

    #[test]
    fn detect_order() {
        let root = std::env::temp_dir().join(format!("blitz-shell-{}", std::process::id()));
        let touch = |p: PathBuf| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        };
        let (on_path, pf, sys) = (root.join("bin"), root.join("pf"), root.join("win"));
        std::fs::create_dir_all(&on_path).unwrap();
        let env = |k: &str| -> Option<OsString> {
            match k {
                "PATH" => Some(std::env::join_paths([&on_path]).unwrap()),
                "ProgramFiles" => Some(pf.clone().into()),
                "SystemRoot" => Some(sys.clone().into()),
                "ComSpec" => Some(r"C:\cmd.exe".into()),
                _ => None,
            }
        };

        assert_eq!(detect_with(env), PathBuf::from(r"C:\cmd.exe"));
        let ps = sys
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        touch(ps.clone());
        assert_eq!(detect_with(env), ps);
        touch(pf.join("PowerShell").join("6").join("pwsh.exe"));
        touch(pf.join("PowerShell").join("7").join("pwsh.exe"));
        std::fs::create_dir_all(pf.join("PowerShell").join("8-preview")).unwrap();
        assert_eq!(
            detect_with(env),
            pf.join("PowerShell").join("7").join("pwsh.exe")
        );
        // A relative entry is resolved against the current directory, so it
        // is never used. Tests run in the crate's folder; this one stays out
        // of the sources.
        let rel = Path::new("..")
            .join("..")
            .join("target")
            .join(format!("blitz-shell-{}", std::process::id()));
        touch(rel.join("pwsh.exe"));
        let with_rel = |k: &str| match k {
            "PATH" => Some(std::env::join_paths([&rel, &on_path]).unwrap()),
            _ => env(k),
        };
        let got = detect_with(with_rel);
        let _ = std::fs::remove_dir_all(&rel);
        assert_eq!(got, pf.join("PowerShell").join("7").join("pwsh.exe"));
        touch(on_path.join("pwsh.exe"));
        assert_eq!(detect_with(env), on_path.join("pwsh.exe"));

        // The settings panel lists only shells that are there.
        touch(pf.join("Git").join("bin").join("bash.exe"));
        let found = installed_with(env);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["PowerShell 7", "Windows PowerShell", "Git Bash"]);
        assert_eq!(found[0].1, on_path.join("pwsh.exe").to_string_lossy());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Folders are listed in name order, which puts `7-preview` after `7`.
    #[test]
    fn detect_prefers_a_release_to_a_preview_of_it() {
        let root = std::env::temp_dir().join(format!("blitz-preview-{}", std::process::id()));
        let pf = root.join("pf");
        for d in ["7", "7-preview", "6"] {
            let p = pf.join("PowerShell").join(d);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("pwsh.exe"), b"").unwrap();
        }
        let env = |k: &str| (k == "ProgramFiles").then(|| pf.clone().into_os_string());
        let got = detect_with(env);
        // A preview of a newer major still wins over an older release.
        std::fs::remove_dir_all(pf.join("PowerShell").join("7")).unwrap();
        let only_preview = detect_with(env);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(got, pf.join("PowerShell").join("7").join("pwsh.exe"));
        assert_eq!(
            only_preview,
            pf.join("PowerShell").join("7-preview").join("pwsh.exe")
        );
    }
}
