//! A program named without a folder never runs from the current directory,
//! which for blitz is wherever Explorer was opened. The only test in this
//! binary: it changes the current directory.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use blitz::pty::{Pty, PtyEvent, SpawnOpts};

/// Runs `cmdline` to completion and returns its exit code.
fn exit_code(cmdline: &str) -> std::io::Result<u32> {
    let (tx, rx) = mpsc::channel();
    let opts = SpawnOpts {
        cmdline,
        cols: 80,
        rows: 24,
        ..Default::default()
    };
    let _pty = Pty::spawn(&opts, move |ev, _| {
        if let PtyEvent::Exit(code) = ev {
            let _ = tx.send(code);
        }
    })?;
    Ok(rx
        .recv_timeout(Duration::from_secs(30))
        .expect("no exit within 30 s"))
}

#[test]
fn bare_programs_never_run_from_the_current_directory() {
    blitz::pty::kill_children_on_exit().expect("job object");
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("planted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let system =
        PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot")).join("System32");
    // Any program that is not cmd will do; this one is on every Windows.
    for name in ["cmd.exe", "blitz-planted.exe"] {
        std::fs::copy(system.join("hostname.exe"), dir.join(name)).expect("plant");
    }
    let back = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&dir).expect("cd");
    // Some shells set this to keep CreateProcessW out of the current
    // directory. blitz started from Explorer has no such luck.
    // SAFETY: the only test in this binary, so nothing else reads the
    // environment while it changes.
    unsafe { std::env::remove_var("NoDefaultCurrentDirectoryInExePath") };

    let real = exit_code("cmd /d /c exit 7");
    let planted = exit_code("blitz-planted");
    let named = exit_code(r".\blitz-planted.exe");

    std::env::set_current_dir(back).expect("cd back");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(real.expect("spawn cmd"), 7, "the system's cmd ran");
    assert_eq!(
        planted
            .expect_err("found only in the current directory")
            .kind(),
        std::io::ErrorKind::NotFound
    );
    // Named with its folder, it is what the user asked for.
    assert!(named.is_ok(), "{named:?}");
}
