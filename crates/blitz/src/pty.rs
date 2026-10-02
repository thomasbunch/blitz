//! ConPTY: loading, spawning, the I/O threads, resize and shutdown.

use std::ffi::OsString;

/// A child process attached to its own pseudoconsole.
pub struct Pty {}

/// Variables that describe the terminal the parent runs in. A child that
/// sees them would believe it runs in that terminal instead of blitz.
const STRIP_PREFIXES: &[&str] = &[
    "TERM_PROGRAM",
    "KITTY_",
    "GHOSTTY_",
    "WEZTERM_",
    "ITERM_",
    "LC_TERMINAL",
    "CONEMU",
    "VSCODE_",
    "ALACRITTY_",
];

const STRIP: &[&str] = &[
    // TERM changes how Git for Windows tools draw; Windows terminals leave
    // it unset.
    "TERM",
    "WT_SESSION",
    "WT_PROFILE_ID",
    "TERMINAL_EMULATOR",
    "VTE_VERSION",
    "ZED_TERM",
    "CURSOR_TRACE_ID",
    "TILIX_ID",
    "KONSOLE_VERSION",
    "GNOME_TERMINAL_SERVICE",
    "TERMINATOR_UUID",
    "XTERM_VERSION",
    "TMUX",
    "STY",
    "COLORTERM",
    // Set when blitz itself was started from a Claude Code session. Each
    // pane starts a fresh session, and the messaging token is a secret.
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_REMOTE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "CLAUDE_PLUGIN_DATA",
    "CLAUDE_PROJECT_DIR",
    "CLAUDE_ENV_FILE",
];

/// The environment for a pane's child: `parent` minus other terminals'
/// markers, plus blitz's own, plus `extra` (which wins). Names compare
/// case-insensitively, and the result is sorted the way Windows expects.
pub fn child_env(
    parent: impl IntoIterator<Item = (OsString, OsString)>,
    pane_id: u32,
    extra: &[(String, String)],
) -> Vec<(OsString, OsString)> {
    let upper = |k: &OsString| k.to_string_lossy().to_ascii_uppercase();
    let mut env: Vec<(OsString, OsString)> = parent
        .into_iter()
        .filter(|(k, _)| {
            let k = upper(k);
            !STRIP.contains(&k.as_str()) && !STRIP_PREFIXES.iter().any(|p| k.starts_with(p))
        })
        .collect();
    let id = pane_id.to_string();
    let ours = [
        ("TERM_PROGRAM", "blitz"),
        ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
        ("COLORTERM", "truecolor"),
        ("FORCE_HYPERLINK", "1"),
        ("BLITZ_PANE_ID", id.as_str()),
    ];
    let sets = ours
        .into_iter()
        .chain(extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    for (k, v) in sets {
        let ku = k.to_ascii_uppercase();
        env.retain(|(e, _)| upper(e) != ku);
        env.push((k.into(), v.into()));
    }
    env.sort_by_cached_key(|(k, _)| upper(k));
    env
}

/// `K=V\0...\0\0` in UTF-16, for `CREATE_UNICODE_ENVIRONMENT`.
pub fn env_block(env: &[(OsString, OsString)]) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut block = Vec::new();
    for (k, v) in env {
        block.extend(k.encode_wide());
        block.push(u16::from(b'='));
        block.extend(v.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_strips_markers_and_sets_ours() {
        let parent = [
            ("CLAUDECODE", "1"),
            ("TERM", "xterm"),
            ("WT_SESSION", "x"),
            ("term_program", "vscode"),
            ("ConEmuPID", "1"),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "secret"),
            ("CLAUDE_CONFIG_DIR", "c"),
            ("ANTHROPIC_MODEL", "m"),
            ("=C:", r"C:\work"),
            ("Path", r"C:\bin"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let extra = [("Path".to_owned(), r"C:\other".to_owned())];
        let env = child_env(parent, 7, &extra);
        let got: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let want = [
            ("=C:", r"C:\work"),
            ("ANTHROPIC_MODEL", "m"),
            ("BLITZ_PANE_ID", "7"),
            ("CLAUDE_CONFIG_DIR", "c"),
            ("COLORTERM", "truecolor"),
            ("FORCE_HYPERLINK", "1"),
            ("Path", r"C:\other"),
            ("TERM_PROGRAM", "blitz"),
            ("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION")),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()));
        assert_eq!(got, want);

        let block = env_block(&env[..1]);
        assert_eq!(String::from_utf16(&block).unwrap(), "=C:=C:\\work\0\0");
    }
}
