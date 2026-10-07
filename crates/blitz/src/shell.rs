//! Default shell detection and shell integration (OSC 133 and OSC 7).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Finds the shell to start when none is configured: `pwsh.exe` on PATH,
/// then the newest `%ProgramFiles%\PowerShell\<n>\pwsh.exe`, then Windows
/// PowerShell, then `%ComSpec%`. Looked up once per run: the PATH walk
/// stats every entry, and each new pane would repeat it.
pub fn detect() -> PathBuf {
    static SHELL: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    SHELL
        .get_or_init(|| detect_with(|k| std::env::var_os(k)))
        .clone()
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
    let found = installed_with(|k| std::env::var_os(k));
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
/// when the program switched.
pub const POWERSHELL_INTEGRATION: &str = r#"if (-not $global:__blitz) {
  $global:__blitz = @{ Orig = $function:prompt; Exec = $false; Token = $env:BLITZ_PANE_TOKEN }
  function global:prompt {
    $ok = $global:?; $code = if ($ok) { 0 } elseif ($global:LASTEXITCODE) { $global:LASTEXITCODE } else { 1 }
    $e = [char]27; $b = [char]7; $s = "$e[?1049h$e[?1049l$e[!p$e[?5W"
    if ($global:__blitz.Exec) { $s += "$e]133;D;$code$b"; $global:__blitz.Exec = $false }
    $s += "$e]133;A;blitz=$($global:__blitz.Token)$b"
    if ($PWD.Provider.Name -eq 'FileSystem') {
      $p = $PWD.ProviderPath -replace '^\\\\\?\\UNC\\', '\\' -replace '^\\\\\?\\', ''
      try { $s += "$e]7;" + [Uri]::new($p).AbsoluteUri + $b } catch {}
    }
    if (-not $ok) { Write-Error 'x' -ErrorAction Ignore }
    $s + (& $global:__blitz.Orig) + "$e]133;B$b"
  }
  if (Get-Module PSReadLine) {
    $global:__blitz.RL = $function:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
      $l = & $global:__blitz.RL; $global:__blitz.Exec = $true
      [Console]::Write("$([char]27)]133;C$([char]7)"); $l
    }
  }
}"#;

/// cmd's prompt with the same marks and reset. cmd cannot report exit
/// codes, nor expand variables in its prompt, so the token is written in.
pub fn cmd_prompt(token: &str) -> String {
    format!(
        r"$e[?1049h$e[?1049l$e[!p$e[?5W$e]133;D$e\$e]133;A;blitz={token}$e\$e]9;9;$P$e\$P$G$e]133;B$e\"
    )
}

/// A command line ready for `CreateProcessW`, plus variables to add to the
/// child's environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub cmdline: String,
    pub env: Vec<(String, String)>,
}

/// Builds the command line for `program` (empty means [`detect`]). Shell
/// integration is added only when `integrate` is set and there are no user
/// arguments; otherwise the command runs exactly as configured. `token` is
/// the pane's `BLITZ_PANE_TOKEN`.
pub fn launch(program: &str, args: &[String], integrate: bool, token: &str) -> Launch {
    let program = if program.is_empty() {
        detect()
    } else {
        PathBuf::from(program)
    };
    let mut out = Launch {
        cmdline: quote(&program.to_string_lossy()),
        env: Vec::new(),
    };
    if integrate && args.is_empty() {
        match kind(&program) {
            Kind::PowerShell => {
                // -EncodedCommand still runs when the execution policy forbids
                // scripts, and the user's profile has already loaded by then.
                let utf16: Vec<u8> = POWERSHELL_INTEGRATION
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect();
                out.cmdline += " -NoLogo -NoExit -EncodedCommand ";
                out.cmdline += &base64(&utf16);
            }
            Kind::Cmd if std::env::var_os("PROMPT").is_none() => {
                out.env.push(("PROMPT".into(), cmd_prompt(token)));
            }
            _ => {}
        }
        return out;
    }
    for a in args {
        out.cmdline.push(' ');
        out.cmdline += &quote(a);
    }
    out
}

pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(c.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(c.get(2).copied().unwrap_or(0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(char::from(T[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        // "hi" as UTF-16LE, the form -EncodedCommand expects.
        assert_eq!(base64(&[b'h', 0, b'i', 0]), "aABpAA==");
    }

    #[test]
    fn prompts_leave_the_alternate_screen() {
        let reset = "$e[?1049h$e[?1049l$e[!p$e[?5W";
        assert!(POWERSHELL_INTEGRATION.contains(&format!("$s = \"{reset}\"")));
        assert!(cmd_prompt("1").starts_with(reset));
        let prompt = cmd_prompt("1").replace("$e", "\x1b");
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

    #[test]
    fn kinds() {
        assert_eq!(kind(Path::new(r"C:\x\PowerShell.exe")), Kind::PowerShell);
        assert_eq!(kind(Path::new("pwsh")), Kind::PowerShell);
        assert_eq!(kind(Path::new("CMD.EXE")), Kind::Cmd);
        assert_eq!(kind(Path::new("bash.exe")), Kind::Other);
    }

    #[test]
    fn launch_integration() {
        let ps = launch(r"C:\Program Files\PowerShell\7\pwsh.exe", &[], true, "t");
        let (head, b64) = ps.cmdline.rsplit_once(' ').unwrap();
        assert_eq!(
            head,
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo -NoExit -EncodedCommand"#
        );
        assert!(b64.len() > 1000 && b64.len() < 8000);
        // User arguments turn integration off.
        let args = ["-NoProfile".to_owned(), "a b".to_owned()];
        assert_eq!(
            launch("pwsh.exe", &args, true, "t").cmdline,
            r#"pwsh.exe -NoProfile "a b""#
        );
        assert_eq!(launch("pwsh.exe", &[], false, "t").cmdline, "pwsh.exe");
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
        // is never used.
        let rel = PathBuf::from(format!("blitz-shell-{}", std::process::id()));
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
