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
    for args in [&["--help"][..], &["-h"], &["/?"]] {
        let out = blitz(Path::new(BLITZ), args, &tmp("cli"));
        assert_eq!(out.status.code(), Some(0), "{args:?}");
        assert!(text(&out.stdout).starts_with("usage: blitz"), "{args:?}");
    }
    // None of these opens a window: each is a mistake, said on the console.
    for (args, why) in [
        (
            &["no-such-folder-here"][..],
            "blitz: no such folder: no-such-folder-here",
        ),
        (
            &["--new-window", "--frob", "1"],
            "blitz: unknown option --frob",
        ),
        (&["--cwd"], "blitz: --cwd needs a value"),
        (&["setup"], ""),
        (&["setup", "vim"], ""),
        (&["debug"], ""),
        (&["debug", "frob"], ""),
        (&["setup", "claude", "--linux"], ""),
    ] {
        let out = blitz(Path::new(BLITZ), args, &tmp("cli"));
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        let err = text(&out.stderr);
        assert!(
            err.starts_with(why) && err.contains("usage: blitz"),
            "{err}"
        );
    }
}

/// Every option blitz reads from its command line is in its usage.
#[test]
fn cli_usage_lists_every_option() {
    let out = blitz(Path::new(BLITZ), &["--help"], &tmp("cli"));
    let usage = text(&out.stdout);
    let sources = [
        include_str!("../src/app.rs"),
        include_str!("../src/debug.rs"),
        include_str!("../src/render/mod.rs"),
    ];
    let mut seen = 0;
    for src in sources {
        // Each option is matched as `"--name" =>` or compared as
        // `== "--name"`.
        for (i, _) in src.match_indices("\"--") {
            let rest = &src[i + 1..];
            let Some(end) = rest[2..].find(|c: char| !c.is_ascii_lowercase() && c != '-') else {
                continue;
            };
            let flag = &rest[..end + 2];
            let read = rest[end + 3..].starts_with(" =>") || src[..i].ends_with("== ");
            if read {
                assert!(usage.contains(flag), "{flag} is not in the usage:\n{usage}");
                seen += 1;
            }
        }
    }
    assert!(seen > 20, "found only {seen} options");
}

/// The shell integration goes to stdout, ready to append to the startup
/// file, and where to put it to stderr.
#[test]
fn cli_setup_shell_prints_the_integration() {
    for (sh, script, rc) in [
        ("bash", blitz::shell::BASH_INTEGRATION, "~/.bashrc"),
        ("zsh", blitz::shell::ZSH_INTEGRATION, "~/.zshrc"),
    ] {
        let out = blitz(Path::new(BLITZ), &["setup", "shell", sh], &tmp("cli"));
        assert_eq!(out.status.code(), Some(0), "{sh}");
        assert_eq!(text(&out.stdout), script);
        // Last, so it marks the prompt the lines before it set.
        let at = format!("the end of {rc}");
        assert!(text(&out.stderr).contains(&at), "{sh}");
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

/// The text a version resource gives for `name`, in US English.
fn version_text(block: &[u8], name: &str) -> String {
    use windows::Win32::Storage::FileSystem::VerQueryValueW;
    let key = windows::core::HSTRING::from(format!(r"\StringFileInfo\040904B0\{name}"));
    let (mut at, mut len) = (std::ptr::null_mut(), 0);
    // SAFETY: `block` came from GetFileVersionInfoW; the out-pointers are
    // valid.
    let found = unsafe { VerQueryValueW(block.as_ptr().cast(), &key, &mut at, &mut len) };
    assert!(found.as_bool() && !at.is_null(), "{name}");
    // SAFETY: the text lies inside `block`, `len` characters long.
    let units = unsafe { std::slice::from_raw_parts(at.cast::<u16>(), len as usize) };
    String::from_utf16_lossy(units)
        .trim_end_matches('\0')
        .to_string()
}

/// Explorer's Details tab and Task Manager name each exe and its version,
/// and both always run as the user who started them.
#[test]
fn exes_carry_version_information_and_a_manifest() {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    use windows::Win32::System::LibraryLoader::{
        FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE, LoadLibraryExW,
        LoadResource, LockResource, SizeofResource,
    };
    use windows::Win32::UI::WindowsAndMessaging::RT_MANIFEST;
    use windows::core::{HSTRING, PCWSTR, w};

    let version = env!("CARGO_PKG_VERSION");
    let hook = env!("CARGO_BIN_EXE_blitz-hook");
    for (exe, name, about) in [
        (BLITZ, "blitz", "blitz"),
        (hook, "blitz-hook", "blitz hook for Claude Code"),
    ] {
        let path = HSTRING::from(exe);
        // SAFETY: a valid path and no handle out.
        let size = unsafe { GetFileVersionInfoSizeW(&path, None) };
        assert!(size > 0, "{exe} has no version information");
        let mut block = vec![0u8; size as usize];
        // SAFETY: `block` holds `size` bytes.
        unsafe { GetFileVersionInfoW(&path, None, size, block.as_mut_ptr().cast()) }
            .expect("GetFileVersionInfoW");
        assert_eq!(version_text(&block, "FileVersion"), version);
        assert_eq!(version_text(&block, "ProductVersion"), version);
        assert_eq!(version_text(&block, "ProductName"), "blitz");
        assert_eq!(version_text(&block, "FileDescription"), about);
        assert_eq!(
            version_text(&block, "OriginalFilename"),
            format!("{name}.exe")
        );
        let (mut at, mut len) = (std::ptr::null_mut(), 0);
        // SAFETY: as in `version_text`.
        let found = unsafe { VerQueryValueW(block.as_ptr().cast(), w!(r"\"), &mut at, &mut len) };
        assert!(found.as_bool() && len >= 52, "{exe}: no fixed version");
        // SAFETY: VS_FIXEDFILEINFO is 13 aligned u32s inside `block`.
        let fixed = unsafe { std::slice::from_raw_parts(at.cast::<u32>(), 13) };
        let n: Vec<u32> = version.split('.').map(|p| p.parse().unwrap()).collect();
        assert_eq!(fixed[0], 0xfeef_04bd, "signature");
        assert_eq!((fixed[2], fixed[3]), (n[0] << 16 | n[1], n[2] << 16));

        // SAFETY: loaded as data only; nothing in it runs.
        let m = unsafe {
            LoadLibraryExW(
                &path,
                None,
                LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
            )
        }
        .expect("load as data");
        // SAFETY: a loaded module; resource 1 is the process manifest.
        let manifest = unsafe {
            let r = FindResourceW(Some(m), PCWSTR(1 as _), RT_MANIFEST);
            assert!(!r.is_invalid(), "{exe} has no manifest");
            let data = LockResource(LoadResource(Some(m), r).expect("LoadResource"));
            let len = SizeofResource(Some(m), r) as usize;
            String::from_utf8_lossy(std::slice::from_raw_parts(data.cast::<u8>(), len)).into_owned()
        };
        // SAFETY: loaded above and no longer used.
        let _ = unsafe { FreeLibrary(m) };
        assert!(manifest.contains(r#"level="asInvoker""#), "{manifest}");
        assert!(manifest.contains("8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a"));
    }
}
