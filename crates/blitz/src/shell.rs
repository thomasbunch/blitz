//! Default shell detection and shell integration (OSC 133 and OSC 7).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// Finds the shell to start when none is configured: `pwsh.exe` on PATH,
/// then the newest `%ProgramFiles%\PowerShell\<n>\pwsh.exe`, then Windows
/// PowerShell, then `%ComSpec%`. Looked up again when a new pane's PATH
/// changed, so one installed while blitz runs is found.
pub fn detect() -> PathBuf {
    static LAST: Memo<Option<OsString>, PathBuf> = Mutex::new(None);
    memo(&LAST, pane_var("PATH"), |path| {
        detect_with(pane_env(path.clone()))
    })
}

/// The last value [`memo`] made, and its key.
type Memo<K, T> = Mutex<Option<(K, T)>>;

/// `make(key)`, kept in `last` and made again only once `key` changed.
/// Looking for a program stats a file in each PATH folder, which takes
/// seconds on a network drive that is not there; the PATH changes when a
/// program is installed.
fn memo<K: PartialEq, T: Clone>(last: &Memo<K, T>, key: K, make: impl FnOnce(&K) -> T) -> T {
    let mut last = last.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((k, v)) = &*last
        && *k == key
    {
        return v.clone();
    }
    let v = make(&key);
    *last = Some((key, v.clone()));
    v
}

/// The environment of a new pane whose PATH is `path`, from [`pane_var`].
pub fn pane_env(path: Option<OsString>) -> impl Fn(&str) -> Option<OsString> {
    move |k| {
        if k.eq_ignore_ascii_case("PATH") {
            path.clone()
        } else {
            std::env::var_os(k)
        }
    }
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

/// PATH for a new pane: `inherited`, blitz's own PATH, in its order, so a
/// developer prompt or a virtual environment blitz was started from still
/// finds its own programs first; then the folders of the system's Path and
/// the user's, as the registry holds them now, that it lacks, so a program
/// installed while blitz runs is found. Just `inherited` when the registry
/// has neither.
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
    for dir in [&inherited, &machine, &user]
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
/// variables in it expanded as a new sign-in would: from the user's and
/// the system's variables as the registry holds them now, else blitz's
/// own. A variable added since blitz started, as `%NVM_HOME%` by an
/// installer, would otherwise stay as written.
#[cfg(windows)]
fn registry_path(machine: bool) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use windows::core::{HSTRING, PCWSTR, w};

    let system = w!(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment");
    let (key, sub) = if machine {
        (HKEY_LOCAL_MACHINE, system)
    } else {
        (HKEY_CURRENT_USER, w!("Environment"))
    };
    let own = |k: &str| std::env::var(k).ok();
    let var = |k: &str| {
        let name = HSTRING::from(k);
        let name = PCWSTR(name.as_ptr());
        (registry_string(HKEY_CURRENT_USER, w!("Environment"), name))
            .or_else(|| registry_string(HKEY_LOCAL_MACHINE, system, name))
            .map(|v| expand(&v, own))
            .or_else(|| own(k))
    };
    Some(expand(&registry_string(key, sub, w!("Path"))?, var))
}

/// `s` with each `%NAME%` that `var` knows replaced by its value, as
/// Windows expands a variable in the registry. One it does not know stays
/// as written.
#[cfg(any(windows, test))]
fn expand(s: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('%') else {
            out.push_str(&rest[i..]);
            return out;
        };
        match var(&after[..j]).filter(|_| j > 0) {
            Some(v) => {
                out.push_str(&v);
                rest = &after[j + 1..];
            }
            // The closing `%` may open the next name.
            None => {
                out.push('%');
                out.push_str(&after[..j]);
                rest = &after[j..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A string value in the registry, as it is stored: variables in a
/// REG_EXPAND_SZ value are left for the caller to expand.
#[cfg(windows)]
fn registry_string(
    key: windows::Win32::System::Registry::HKEY,
    sub: windows::core::PCWSTR,
    value: windows::core::PCWSTR,
) -> Option<String> {
    use windows::Win32::Foundation::ERROR_MORE_DATA;
    use windows::Win32::System::Registry::{
        RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
    };

    let mut buf = vec![0u16; 2048];
    // The value can grow between the size query and the read.
    for _ in 0..4 {
        let mut size = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `size` bytes; both outlive the call.
        let r = unsafe {
            RegGetValueW(
                key,
                sub,
                value,
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
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

/// The names of the WSL distributions installed for this user.
#[cfg(windows)]
fn wsl_distros() -> Vec<String> {
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;
    use windows::core::w;

    let lxss = w!(r"Software\Microsoft\Windows\CurrentVersion\Lxss");
    subkey_strings(HKEY_CURRENT_USER, lxss, w!("DistributionName"))
}

/// The string `value` of each key under `path` that has one.
#[cfg(windows)]
fn subkey_strings(
    root: windows::Win32::System::Registry::HKEY,
    path: windows::core::PCWSTR,
    value: windows::core::PCWSTR,
) -> Vec<String> {
    use windows::Win32::System::Registry::{
        HKEY, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW,
    };
    use windows::core::{PCWSTR, PWSTR};

    let mut key = HKEY::default();
    // SAFETY: a valid out pointer; the key is closed below.
    if unsafe { RegOpenKeyExW(root, path, None, KEY_READ, &mut key) }.is_err() {
        return Vec::new();
    }
    let mut names = Vec::new();
    // Key names are at most 255 characters.
    let mut sub = [0u16; 256];
    for i in 0.. {
        let mut len = sub.len() as u32;
        // SAFETY: `sub` holds `len` characters, and the name comes back
        // ended with a NUL.
        let r = unsafe {
            RegEnumKeyExW(
                key,
                i,
                Some(PWSTR(sub.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if r.is_err() {
            break;
        }
        names.extend(registry_string(key, PCWSTR(sub.as_ptr()), value));
    }
    // SAFETY: opened above and not used after.
    let _ = unsafe { RegCloseKey(key) };
    names
}

#[cfg(not(windows))]
fn wsl_distros() -> Vec<String> {
    Vec::new()
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
    absolute(var("ComSpec")).unwrap_or_else(|| root.join("System32").join("cmd.exe"))
}

/// The Windows folder, from `SystemRoot`. A value that is empty or not an
/// absolute path is ignored: it would name a folder under the current one,
/// where a planted program would run.
pub(crate) fn system_root(var: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    absolute(var("SystemRoot")).unwrap_or_else(|| r"C:\Windows".into())
}

/// `v` as a path, unless Windows would read it against the current folder.
/// Elsewhere a Windows path is never absolute, and nothing runs from it.
fn absolute(v: Option<OsString>) -> Option<PathBuf> {
    v.map(PathBuf::from)
        .filter(|p| cfg!(not(windows)) || p.is_absolute())
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

/// The shells the settings panel offers, as (name, `shell` setting): first
/// the automatic choice, whose setting is empty, then each one installed.
pub fn choices() -> Vec<(String, String)> {
    type Found = Vec<(String, String)>;
    static LAST: Memo<(Option<OsString>, Vec<String>), Found> = Mutex::new(None);
    let key = (pane_var("PATH"), wsl_distros());
    let found = memo(&LAST, key, |(path, distros)| {
        installed_with(pane_env(path.clone()), distros.clone())
    });
    let auto = detect();
    let name = (found.iter())
        .find(|(_, p)| Path::new(p) == auto)
        .map_or_else(|| auto.to_string_lossy().into_owned(), |(n, _)| n.clone());
    let mut out = vec![(format!("Automatic ({name})"), String::new())];
    out.extend(found);
    out
}

/// Shells found with the environment supplied by the caller, and one for
/// each of the WSL distributions `distros`, as (name, `shell` setting).
/// Docker and Rancher Desktop's own distributions are no shells, and
/// Windows Terminal leaves them out too.
fn installed_with(
    var: impl Fn(&str) -> Option<OsString>,
    mut distros: Vec<String>,
) -> Vec<(String, String)> {
    let root = system_root(&var);
    let sys = root.join("System32");
    let found = |exe: Option<PathBuf>| {
        let exe = exe.filter(|e| e.is_file())?;
        Some(exe.to_string_lossy().into_owned())
    };
    let mut out: Vec<(String, String)> = [
        ("PowerShell 7", pwsh(&var)),
        ("Windows PowerShell", Some(windows_powershell(&root))),
        ("Command Prompt", Some(sys.join("cmd.exe"))),
    ]
    .into_iter()
    .filter_map(|(name, exe)| Some((name.to_string(), found(exe)?)))
    .collect();
    // A login shell reads /etc/profile, which sets up Git's tools and prompt.
    if let Some(bash) = found(git_bash(&var)) {
        out.push(("Git Bash".into(), quote(&bash) + " --login -i"));
    }
    if let Some(wsl) = found(Some(sys.join("wsl.exe"))) {
        let theirs = |d: &str| {
            ["docker-desktop", "rancher-desktop"]
                .iter()
                .any(|t| d.starts_with(t))
        };
        distros.retain(|d| !d.is_empty() && !theirs(&d.to_ascii_lowercase()));
        distros.sort_by_key(|d| d.to_lowercase());
        for d in distros {
            let shell = format!("{} -d {}", quote(&wsl), quote(&d));
            out.push((d, shell));
        }
    }
    out
}

/// Git's `bin\bash.exe`: of the Git whose `git.exe` is on PATH, as one
/// installed for this user only is, else of `%ProgramFiles%\Git`.
fn git_bash(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let path = var("PATH").unwrap_or_default();
    // Git puts its `cmd` folder on PATH, and may put `mingw64\bin`.
    let on_path: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|d| d.is_absolute() && d.join("git.exe").is_file())
        .flat_map(|d| {
            d.ancestors()
                .skip(1)
                .take(2)
                .map(Path::to_path_buf)
                .collect::<Vec<_>>()
        })
        .collect();
    let pf = var("ProgramFiles").map(|pf| Path::new(&pf).join("Git"));
    (on_path.into_iter().chain(pf))
        .map(|git| git.join("bin").join("bash.exe"))
        .find(|bash| bash.is_file())
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

/// The same marks, reset and folder for bash, in Git Bash or WSL, which
/// `blitz setup shell bash` prints for `~/.bashrc`. It runs last among the
/// prompt commands, so a prompt they set still gets its B mark. Only a
/// Windows folder is reported: Git Bash's, or one under WSL's `/mnt`. A
/// drive's folder is worked out without starting `pwd -W`, which takes
/// tens of milliseconds under MSYS. Read again, as by `source ~/.bashrc`,
/// it puts its hooks back first and last, and never twice.
/// Needs bash 4.4 for `PS0`; from bash 5.1 `PROMPT_COMMAND` may be an
/// array, each of whose commands runs.
pub const BASH_INTEGRATION: &str = r#"# blitz shell integration: marks each prompt and reports the folder.
# It does nothing outside blitz.
if [ -n "$BLITZ_PANE_TOKEN" ]; then
  __blitz_code=${__blitz_code:-0}
  __blitz_status() { __blitz_code=$?; return $__blitz_code; }
  __blitz_prompt() {
    local dir=
    printf '\e[?1049h\e[?1049l\e[!p\e[?5W'
    [ -n "$__blitz_ran" ] && printf '\e]133;D;%s\a' "$__blitz_code"
    __blitz_ran=
    printf '\e]133;A;blitz=%s\a' "$BLITZ_PANE_TOKEN"
    case $PWD in
      /mnt/[a-z]|/mnt/[a-z]/*) dir=${PWD:5:1}:/${PWD:7} ;;
      # As `pwd -W` says it, with the drive in capitals.
      /[a-z]|/[a-z]/*) [ -n "$MSYSTEM" ] && dir=${PWD:1:1} && dir=${dir^}:/${PWD:3} ;;
      *) [ -n "$MSYSTEM" ] && dir=$(pwd -W) ;;
    esac
    [ -n "$dir" ] && printf '\e]7;file:///%s\a' "${dir//\%/%25}"
    case $PS1 in *'\e]133;B'*) ;; *) PS1=$PS1'\[\e]133;B\a\]' ;; esac
    return $__blitz_code
  }
  # First and last, taken out and put back each time, as an rc read again
  # may have put its own around them. Lines, not `;`, as the commands
  # there may end with one.
  if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a'* ]]; then
    __blitz_pc=()
    for __blitz_c in "${PROMPT_COMMAND[@]}"; do
      case $__blitz_c in __blitz_status|__blitz_prompt) ;; *) __blitz_pc+=("$__blitz_c") ;; esac
    done
    PROMPT_COMMAND=(__blitz_status "${__blitz_pc[@]}" __blitz_prompt)
  else
    __blitz_pc=${PROMPT_COMMAND//__blitz_status$'\n'/}
    __blitz_pc=${__blitz_pc//$'\n'__blitz_prompt/}
    [ "$__blitz_pc" = __blitz_prompt ] && __blitz_pc=
    PROMPT_COMMAND=__blitz_status$'\n'${__blitz_pc:+$__blitz_pc$'\n'}__blitz_prompt
  fi
  unset __blitz_pc __blitz_c
  # When a command starts. The arithmetic sets the flag and prints nothing.
  case $PS0 in
    *'133;C'*) ;;
    *) PS0=$PS0'\e]133;C\a${__blitz_ran:0:$((__blitz_ran = 1, 0))}' ;;
  esac
fi
"#;

/// [`BASH_INTEGRATION`] for zsh, which `blitz setup shell zsh` prints for
/// `~/.zshrc`.
pub const ZSH_INTEGRATION: &str = r#"# blitz shell integration: marks each prompt and reports the folder.
# It does nothing outside blitz.
if [[ -n $BLITZ_PANE_TOKEN && -z $__blitz_ran ]]; then
  __blitz_ran=0
  __blitz_precmd() {
    local code=$? dir=
    print -n '\e[?1049h\e[?1049l\e[!p\e[?5W'
    (( __blitz_ran )) && print -n "\e]133;D;$code\a"
    __blitz_ran=0
    print -n "\e]133;A;blitz=$BLITZ_PANE_TOKEN\a"
    case $PWD in
      /mnt/[a-z]|/mnt/[a-z]/*) dir=${PWD[6]}:/${PWD[8,-1]} ;;
      /[a-z]|/[a-z]/*) [[ -n $MSYSTEM ]] && dir=${(U)PWD[2]}:/${PWD[4,-1]} ;;
      *) [[ -n $MSYSTEM ]] && dir=$(cygpath -m $PWD) ;;
    esac
    [[ -n $dir ]] && print -n "\e]7;file:///${dir//\%/%25}\a"
    [[ $PS1 == *'133;B'* ]] || PS1=$PS1$'%{\e]133;B\a%}'
    return $code
  }
  __blitz_preexec() { __blitz_ran=1; print -n '\e]133;C\a'; }
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd __blitz_precmd
  add-zsh-hook preexec __blitz_preexec
fi
"#;

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

/// The words of `args` as `CommandLineToArgvW` splits them, so that
/// `"C:\My Projects"` is one, though without its `\"`.
fn words(args: &str) -> Vec<String> {
    let (mut out, mut word, mut quoted) = (Vec::new(), None::<String>, false);
    for c in args.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                word.get_or_insert_default();
            }
            ' ' | '\t' if !quoted => out.extend(word.take()),
            c => word.get_or_insert_default().push(c),
        }
    }
    out.extend(word);
    out
}

/// Whether PowerShell `args` already say what to run, which the
/// integration's own `-Command` would take the place of. PowerShell takes
/// any start of a parameter's name, and `-e` is `-EncodedCommand`. A word
/// that is no parameter, nor a parameter's value, runs too: PowerShell 7
/// takes it as `-File`, Windows PowerShell as `-Command`.
fn runs_command(args: &str) -> bool {
    let runs = ["command", "file", "encodedcommand", "commandwithargs"];
    let valued = [
        "configurationname",
        "configurationfile",
        "custompipename",
        "encodedarguments",
        "executionpolicy",
        "inputformat",
        "outputformat",
        "psconsolefile",
        "settingsfile",
        "version",
        "windowstyle",
        "workingdirectory",
    ];
    let mut words = words(args).into_iter();
    while let Some(w) = words.next() {
        let Some(p) = w.strip_prefix(['-', '/']) else {
            return true;
        };
        // `-wd:C:\x` holds its value.
        let (p, value) = match p.trim_start_matches('-').split_once(':') {
            Some((p, _)) => (p.to_ascii_lowercase(), true),
            None => (p.trim_start_matches('-').to_ascii_lowercase(), false),
        };
        if p.is_empty() {
            continue;
        }
        if runs.iter().any(|n| n.starts_with(&p)) || p == "ec" || p == "cwa" {
            return true;
        }
        // Shorter starts are switches too, such as `-i` for -Interactive.
        let aliases = ["ea", "ep", "ex", "if", "o", "of", "v", "w", "wd"];
        let named = p.len() > 2 && valued.iter().any(|n| n.starts_with(&p));
        if !value && (named || aliases.contains(&p.as_str())) {
            words.next();
        }
    }
    false
}

/// A command line ready for `CreateProcessW`, plus variables to add to the
/// child's environment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub cmdline: String,
    pub env: Vec<(String, String)>,
}

/// The program and arguments of a `shell` setting. A path to a program
/// that holds spaces needs no quotes, as the setting once took only a path:
/// the longest start of it, up to a space, that names a program is the
/// program, and any arguments may follow. A program has its extension: a
/// start cut at a space, such as `D:\My` of a missing `D:\My Tools\nu.exe`,
/// has none, so a file put there is never run. An unquoted path that names
/// no program is all program, as it always was: cut at its first space, it
/// would reach `CreateProcessW` unquoted, which then tries `C:\Program.exe`
/// and the like.
fn parts(shell: &str) -> (&str, &str) {
    let shell = shell.trim();
    let spaces = shell.rmatch_indices([' ', '\t']).map(|m| m.0);
    // `\Program Files\...`, with no drive, is a path too, and so is
    // `tools\nu.exe`, from the current folder as `CreateProcessW` reads it.
    let path = (shell.split([' ', '\t']).next()).is_some_and(|w| w.contains(['\\', '/']));
    let program = |&i: &usize| {
        let p = Path::new(&shell[..i]);
        let ext = p.extension().unwrap_or_default();
        let runs = ["exe", "com", "bat", "cmd"]
            .iter()
            .any(|e| ext.eq_ignore_ascii_case(e));
        path && runs && p.is_file()
    };
    let unquoted = !shell.starts_with('"');
    match (std::iter::once(shell.len()).chain(spaces)).find(|i| unquoted && program(i)) {
        Some(i) => shell.split_at(i),
        None if unquoted && path => (shell, ""),
        None => split_program(shell),
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
/// configured. `token` is the pane's `BLITZ_PANE_TOKEN`, and `env` the
/// variables the settings add, whose PROMPT cmd's marks wrap rather than
/// blitz's own.
pub fn launch(shell: &str, integrate: bool, token: &str, env: &[(String, String)]) -> Launch {
    let (program, args) = match parts(shell) {
        ("", args) => (detect(), args),
        (program, args) => (PathBuf::from(program), args),
    };
    // `"pwsh"-NoLogo` names the program apart; quoted again, it needs a
    // space to.
    let gap = if args.is_empty() || args.starts_with([' ', '\t']) {
        ""
    } else {
        " "
    };
    let mut out = Launch {
        cmdline: quote(&program.to_string_lossy()) + gap + args,
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
                let set = (env.iter().rev()).find(|e| e.0.eq_ignore_ascii_case("PROMPT"));
                let own = match set {
                    Some(e) => Some(e.1.clone()),
                    None => std::env::var_os("PROMPT").map(|p| p.to_string_lossy().into_owned()),
                };
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

    /// A Windows folder or command shell named relative to the current
    /// folder is never used: a program planted there would run.
    #[test]
    #[cfg(windows)]
    fn relative_system_folders_are_ignored() {
        let with =
            |root: &'static str| move |k: &str| (k == "SystemRoot").then(|| OsString::from(root));
        assert_eq!(system_root(with(r"D:\Win")), Path::new(r"D:\Win"));
        for bad in ["", ".", r"System\..", r"\Windows", "C:Windows"] {
            assert_eq!(system_root(with(bad)), Path::new(r"C:\Windows"), "{bad:?}");
        }
        let none = PathBuf::from(r"Z:\no\such");
        let spec = |c: &'static str| {
            let none = none.clone();
            move |k: &str| match k {
                "SystemRoot" => Some(none.clone().into()),
                "ComSpec" => Some(c.into()),
                _ => None,
            }
        };
        assert_eq!(
            detect_with(spec(r"cmd.exe")),
            none.join(r"System32\cmd.exe")
        );
        assert_eq!(detect_with(spec(r"D:\cmd.exe")), Path::new(r"D:\cmd.exe"));
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
    fn bash_and_zsh_mark_prompts_only_in_blitz() {
        for (sh, script) in [("bash", BASH_INTEGRATION), ("zsh", ZSH_INTEGRATION)] {
            let body: Vec<&str> = (script.lines()).filter(|l| !l.starts_with('#')).collect();
            assert!(
                body[0].starts_with("if [") && body[0].contains("-n"),
                "{sh}"
            );
            assert!(body[0].contains("$BLITZ_PANE_TOKEN"), "{sh}");
            assert_eq!(body.last(), Some(&"fi"), "{sh}");
            for mark in [
                r"\e[?1049h\e[?1049l\e[!p\e[?5W",
                r"\e]133;A;blitz=",
                r"\e]133;B\a",
                r"\e]133;C\a",
                r"\e]133;D;",
                r"\e]7;file:///",
            ] {
                assert!(script.contains(mark), "{sh}: {mark}");
            }
        }
    }

    /// `source ~/.bashrc` sets PROMPT_COMMAND anew, and bash 5.1 runs
    /// every command of an array.
    #[test]
    #[cfg(windows)]
    fn bash_hooks_come_back_once_and_run_last() {
        let Some(bash) = git_bash(|k| std::env::var_os(k)) else {
            eprintln!("SKIPPED: no Git Bash, so the bash integration is untested");
            return;
        };
        let file = std::env::temp_dir().join(format!("blitz-rc-{}.sh", std::process::id()));
        std::fs::write(&file, BASH_INTEGRATION).unwrap();
        let script = r#"PROMPT_COMMAND='history -a'; . "$1"; . "$1"; PROMPT_COMMAND='history -a'; . "$1"
declare -p PROMPT_COMMAND; PROMPT_COMMAND="history -a${PROMPT_COMMAND:+; $PROMPT_COMMAND}"; . "$1"
declare -p PROMPT_COMMAND; unset PROMPT_COMMAND; PROMPT_COMMAND=(one two); . "$1"; . "$1"
PROMPT_COMMAND+=(three); . "$1"; declare -p PROMPT_COMMAND; echo "${PS0//[^C]}""#;
        let out = std::process::Command::new(bash)
            .args(["--norc", "-c", script, "x"])
            .arg(&file)
            .env("BLITZ_PANE_TOKEN", "t")
            .output()
            .expect("bash");
        let _ = std::fs::remove_file(&file);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "declare -- PROMPT_COMMAND=$'__blitz_status\\nhistory -a\\n__blitz_prompt'\n\
             declare -- PROMPT_COMMAND=$'__blitz_status\\nhistory -a; history -a\\n__blitz_prompt'\n\
             declare -a PROMPT_COMMAND=([0]=\"__blitz_status\" [1]=\"one\" [2]=\"two\" [3]=\"three\" [4]=\"__blitz_prompt\")\n\
             C\n"
        );
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
        // One the settings set is the one wrapped.
        let set = [("prompt".to_string(), "$T$G".to_string())];
        let env = launch("cmd.exe", true, "5eed", &set).env;
        assert_eq!(env, [("PROMPT".into(), cmd_prompt("5eed", "$T$G"))]);
    }

    #[test]
    fn other_shells_run_as_configured() {
        let bash = r#""C:\Program Files\Git\bin\bash.exe" --login -i"#;
        let want = Launch {
            cmdline: bash.into(),
            env: Vec::new(),
        };
        assert_eq!(launch(bash, true, "t", &[]), want);
        assert_eq!(launch("wsl.exe", true, "t", &[]).cmdline, "wsl.exe");
        assert_eq!(
            launch(" wsl.exe -d Ubuntu ", true, "t", &[]).cmdline,
            "wsl.exe -d Ubuntu"
        );
        assert_eq!(launch(r#""bash"-i"#, true, "t", &[]).cmdline, "bash -i");
        // The shell blitz finds, with the arguments given.
        let auto = launch(r#""" -x"#, false, "t", &[]).cmdline;
        assert!(auto.ends_with(" -x") && auto.len() > 3, "{auto}");
    }

    #[test]
    fn new_panes_get_the_path_the_registry_holds_now() {
        let s = |v: &str| Some(v.to_string());
        // Git installed since blitz started is in the registry only, and a
        // developer prompt's folder only in blitz's own PATH, where it
        // comes first: its link.exe, not Git's.
        assert_eq!(
            fresh_path(
                s(r"C:\Windows\system32;C:\Program Files\Git\usr\bin"),
                s(r"C:\Users\me\.cargo\bin;"),
                s(r"C:\VS\bin;C:\WINDOWS\System32\;;C:\Users\me\.cargo\bin"),
            ),
            s(
                r"C:\VS\bin;C:\WINDOWS\System32\;C:\Users\me\.cargo\bin;C:\Program Files\Git\usr\bin"
            )
        );
        assert_eq!(fresh_path(None, None, s("a;b")), s("a;b"));
        assert_eq!(fresh_path(None, s("u"), None), s("u"));
        assert_eq!(fresh_path(None, None, None), None);
    }

    /// As a sign-in would, from variables blitz's own environment may not
    /// have yet.
    #[test]
    fn registry_variables_are_expanded_by_the_caller() {
        let var = |k: &str| match k {
            "NVM_HOME" => Some(r"C:\nvm".to_string()),
            "A" => Some("a".into()),
            _ => None,
        };
        assert_eq!(
            expand(r"%NVM_HOME%;%nope%;C:\x", var),
            r"C:\nvm;%nope%;C:\x"
        );
        assert_eq!(expand("100%%A%%", var), "100%a%");
        assert_eq!(expand("%A", var), "%A");
        assert_eq!(expand("%%%", var), "%%%");
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

    /// As WSL's distributions are read: a value from each key under one.
    /// Every Windows has services, and many run in svchost.
    #[test]
    #[cfg(windows)]
    fn values_are_read_from_each_key_under_one() {
        use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use windows::core::w;
        let services = w!(r"SYSTEM\CurrentControlSet\Services");
        let paths = subkey_strings(HKEY_LOCAL_MACHINE, services, w!("ImagePath"));
        assert!(paths.len() > 10, "{paths:?}");
        assert!(
            paths
                .iter()
                .any(|p| p.to_lowercase().contains("svchost.exe"))
        );
        let none = w!(r"Software\blitz-no-such-key-4b1d");
        assert!(subkey_strings(HKEY_CURRENT_USER, none, w!("x")).is_empty());
    }

    /// The setting once took only a path, written without quotes.
    #[test]
    fn a_path_with_spaces_is_one_program() {
        let dir = std::env::temp_dir().join(format!("blitz shell {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sh = dir.join("sh.exe");
        std::fs::write(&sh, b"").unwrap();
        let got = launch(&sh.to_string_lossy(), true, "t", &[]).cmdline;
        // Arguments may follow it, and it still runs as one program.
        let args = format!("{} --login -i", sh.display());
        let (with, shown) = (launch(&args, true, "t", &[]).cmdline, label(&args));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, format!("\"{}\"", sh.display()));
        assert_eq!(with, format!("\"{}\" --login -i", sh.display()));
        assert_eq!(shown, "sh.exe --login -i");
    }

    /// A program from the current folder takes arguments too.
    #[test]
    fn a_relative_path_takes_arguments() {
        let dir = format!("blitz-rel-{}", std::process::id());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Path::new(&dir).join("nu.exe"), b"").unwrap();
        let (program, shell) = (format!("{dir}/nu.exe"), format!("{dir}/nu.exe -l"));
        let got = parts(&shell);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, (program.as_str(), " -l"));
    }

    /// Cut at its first space, a missing program would run as the first
    /// word of its path, which another user may have put there.
    #[test]
    fn a_missing_path_with_spaces_stays_quoted() {
        let dir = std::env::temp_dir().join(format!("blitz no such {}", std::process::id()));
        let shell = format!("{} -i", dir.join("sh").display());
        assert_eq!(launch(&shell, true, "t", &[]).cmdline, quote(&shell));
        // Without its drive, and from the current folder.
        for shell in [
            r"\Program Files\blitz no such\sh.exe -l",
            r"tools\no such\sh -i",
        ] {
            assert_eq!(launch(shell, true, "t", &[]).cmdline, quote(shell));
        }
    }

    /// A file put where a missing program's path is cut is not run.
    #[test]
    fn a_file_at_the_start_of_a_missing_path_is_not_the_program() {
        let dir = std::env::temp_dir().join(format!("blitz plant {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("My"), b"").unwrap();
        let shell = format!("{} -l", dir.join("My Tools").join("nu.exe").display());
        let got = launch(&shell, true, "t", &[]).cmdline;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, quote(&shell));
    }

    #[test]
    fn powershell_arguments_stay_and_integration_follows_them() {
        let ps = |s: &str| launch(s, true, "t", &[]).cmdline;
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
            r"pwsh C:\s\start.ps1",
            "powershell Get-Date",
            "pwsh -NoProfile -wd C: x.ps1",
        ] {
            assert_eq!(ps(s), s);
        }
        // A parameter's value is not what to run.
        for s in [
            "pwsh -wd c",
            "pwsh -WorkingDirectory c",
            "pwsh -wd:c -ex f",
            r#"pwsh -wd "C:\My Projects" -NoProfile"#,
            r#"pwsh -SettingsFile "C:\Program Files\x.json""#,
        ] {
            assert!(ps(s).contains(" -NoExit -Command "), "{s}");
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

    /// Each new pane and each palette would otherwise walk PATH again.
    #[test]
    fn shells_are_looked_for_again_only_when_the_path_changes() {
        let last = Mutex::new(None);
        let walks = std::cell::Cell::new(0);
        let look = |path: &str| {
            memo(&last, path.to_string(), |p| {
                walks.set(walks.get() + 1);
                p.len()
            })
        };
        assert_eq!((look("a;b"), look("a;b"), walks.get()), (3, 3, 1));
        assert_eq!((look("a;b;c"), walks.get()), (5, 2));
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
                .all(|(n, p)| !n.is_empty() && Path::new(parts(p).0).is_file()),
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
        let ps = launch(
            r#""C:\Program Files\PowerShell\7\pwsh.exe""#,
            true,
            "t",
            &[],
        );
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
        assert_eq!(launch("pwsh.exe", false, "t", &[]).cmdline, "pwsh.exe");
    }

    /// The script reaches PowerShell as one argument, as written.
    #[test]
    #[cfg(windows)]
    fn the_powershell_script_is_one_argument() {
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        let line = windows::core::HSTRING::from(launch("pwsh", true, "t", &[]).cmdline);
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
        let found = installed_with(env, vec!["Ubuntu".into()]);
        let names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["PowerShell 7", "Windows PowerShell", "Git Bash"]);
        assert_eq!(found[0].1, on_path.join("pwsh.exe").to_string_lossy());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn git_bash_logs_in_and_each_wsl_distribution_is_a_shell() {
        let root = std::env::temp_dir().join(format!("blitz-shells-{}", std::process::id()));
        let touch = |p: &Path| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        };
        let (pf, sys) = (root.join("pf"), root.join("win"));
        // Git for this user only, as its installer can put it.
        let mine = root.join("Programs").join("Git");
        let wsl = sys.join("System32").join("wsl.exe");
        for f in [
            &pf.join("Git").join("bin").join("bash.exe"),
            &mine.join("cmd").join("git.exe"),
            &mine.join("bin").join("bash.exe"),
            &wsl,
        ] {
            touch(f);
        }
        let env = |path: &Path| {
            let path = std::env::join_paths([path]).unwrap();
            let (pf, sys) = (pf.clone(), sys.clone());
            move |k: &str| match k {
                "PATH" => Some(path.clone()),
                "ProgramFiles" => Some(pf.clone().into_os_string()),
                "SystemRoot" => Some(sys.clone().into_os_string()),
                _ => None,
            }
        };
        let distros = [
            "Ubuntu",
            "docker-desktop-data",
            "debian",
            "Rancher-Desktop",
            "",
        ];
        let found = installed_with(env(&mine.join("cmd")), distros.map(Into::into).to_vec());
        let elsewhere = installed_with(env(&root), Vec::new());
        let _ = std::fs::remove_dir_all(&root);

        let bash = |git: &Path| quote(&git.join("bin").join("bash.exe").to_string_lossy());
        let wsl = quote(&wsl.to_string_lossy());
        let want = [
            ("Git Bash", bash(&mine) + " --login -i"),
            ("debian", format!("{wsl} -d debian")),
            ("Ubuntu", format!("{wsl} -d Ubuntu")),
        ]
        .map(|(n, s)| (n.to_string(), s));
        assert_eq!(found, want);
        let git = (elsewhere.iter()).find(|s| s.0 == "Git Bash").map(|s| &s.1);
        assert_eq!(git, Some(&(bash(&pf.join("Git")) + " --login -i")));
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
