//! Runs the `blitz` binary's subcommands. Under `cargo test --release` it
//! is the GUI-subsystem exe that ships, whose output must still reach a
//! pipe.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use blitz::hook::Json;

const BLITZ: &str = env!("CARGO_BIN_EXE_blitz");

fn blitz(exe: &Path, args: &[&str], config: &Path) -> Output {
    Command::new(exe)
        .args(args)
        .env("CLAUDE_CONFIG_DIR", config)
        .stdin(Stdio::null())
        .output()
        .expect("run blitz")
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).expect("UTF-8 output")
}

fn tmp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

#[test]
fn cli_prints_its_version() {
    let out = blitz(Path::new(BLITZ), &["--version"], &tmp("cli"));
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout).trim_end(),
        concat!("blitz ", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn cli_prints_usage() {
    for args in [&["--help"][..], &["-h"]] {
        let out = blitz(Path::new(BLITZ), args, &tmp("cli"));
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(text(&out.stdout).starts_with("usage: blitz"), "{args:?}");
    }
    for args in [
        &["bogus"][..],
        &["setup"],
        &["setup", "vim"],
        &["setup", "claude", "--linux"],
        &["debug"],
        &["debug", "frob"],
    ] {
        let out = blitz(Path::new(BLITZ), args, &tmp("cli"));
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(text(&out.stderr).contains("usage: blitz"), "{args:?}");
    }
}

/// The settings name the `blitz-hook` next to this exe, and the message
/// names the settings file Claude Code reads.
#[test]
fn cli_setup_claude_prints_the_hooks() {
    let config = tmp("cli-claude-config");
    let out = blitz(Path::new(BLITZ), &["setup", "claude"], &config);
    assert_eq!(out.status.code(), Some(0));
    let v = Json::parse(&text(&out.stdout)).expect("settings JSON");
    let hook = Path::new(BLITZ).with_file_name("blitz-hook.exe");
    let stop = v
        .get("hooks")
        .and_then(|h| h.get("Stop"))
        .expect("a Stop hook");
    let Json::Arr(groups) = stop else {
        panic!("{stop:?}")
    };
    let Some(Json::Arr(cmds)) = groups[0].get("hooks") else {
        panic!("{stop:?}")
    };
    assert_eq!(
        cmds[0].get("command").and_then(Json::as_str),
        Some(hook.to_str().expect("UTF-8 path"))
    );
    let err = text(&out.stderr);
    assert!(
        err.contains(&config.join("settings.json").display().to_string()),
        "{err}"
    );
    assert!(!err.contains("is missing"), "{err}");
    assert!(err.contains("nothing to set up"), "{err}");
    assert!(!err.contains("runs blitz-hook already"), "{err}");
}

/// Claude Code inside WSL runs the same blitz-hook, by its WSL path.
#[test]
fn cli_setup_claude_for_wsl() {
    let out = blitz(Path::new(BLITZ), &["setup", "claude", "--wsl"], &tmp("cli"));
    assert_eq!(out.status.code(), Some(0));
    let v = Json::parse(&text(&out.stdout)).expect("settings JSON");
    let Some(Json::Arr(groups)) = v.get("hooks").and_then(|h| h.get("Stop")) else {
        panic!("{v:?}")
    };
    let Some(Json::Arr(cmds)) = groups[0].get("hooks") else {
        panic!("{v:?}")
    };
    let cmd = cmds[0].get("command").and_then(Json::as_str).unwrap();
    let hook = Path::new(BLITZ).with_file_name("blitz-hook.exe");
    let hook = hook.to_str().expect("UTF-8 path");
    let want = format!(
        "/mnt/{}/{}",
        hook[..1].to_ascii_lowercase(),
        hook[3..].replace('\\', "/")
    );
    assert_eq!(cmd, want);
    assert!(text(&out.stderr).contains("inside WSL"));
}

/// Hooks pasted before blitz loaded its own are pointed out: with both,
/// every event reports twice.
#[test]
fn cli_setup_claude_flags_pasted_hooks() {
    let config = tmp("cli-claude-pasted");
    std::fs::create_dir_all(&config).expect("dir");
    let settings = config.join("settings.json");
    std::fs::write(
        &settings,
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"C:\\Tools\\Blitz-Hook.exe","args":["claude"]}]}]}}"#,
    )
    .expect("settings");
    let out = blitz(Path::new(BLITZ), &["setup", "claude"], &config);
    assert_eq!(out.status.code(), Some(0));
    let err = text(&out.stderr);
    assert!(
        err.contains(&format!("{} runs blitz-hook already", settings.display())),
        "{err}"
    );
    std::fs::write(&settings, r#"{"model":"opus"}"#).expect("settings");
    let out = blitz(Path::new(BLITZ), &["setup", "claude"], &config);
    assert!(!text(&out.stderr).contains("already"));
}

#[test]
fn cli_setup_claude_warns_without_the_hook() {
    let dir = tmp("cli-alone");
    std::fs::create_dir_all(&dir).expect("dir");
    let exe = dir.join("blitz.exe");
    std::fs::copy(BLITZ, &exe).expect("copy blitz");
    let out = blitz(&exe, &["setup", "claude"], &tmp("cli"));
    assert_eq!(out.status.code(), Some(0));
    let err = text(&out.stderr);
    assert!(
        err.contains(&format!(
            "{} is missing",
            dir.join("blitz-hook.exe").display()
        )),
        "{err}"
    );
    assert!(Json::parse(&text(&out.stdout)).is_some());
}
