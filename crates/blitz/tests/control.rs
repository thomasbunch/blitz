//! `blitz pane` against a stand-in for blitz that answers with lines fixed
//! in advance: run as blitz itself, and as the copy of blitz-hook that
//! blitz puts on every pane's PATH.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use blitz::control::{Op, error, parse};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn run(exe: &Path, args: &[&str], pipe: &str, token: Option<&str>) -> Output {
    let mut c = Command::new(exe);
    c.args(args).env("BLITZ_PIPE", pipe).stdin(Stdio::null());
    match token {
        Some(t) => c.env("BLITZ_PANE_TOKEN", t),
        None => c.env_remove("BLITZ_PANE_TOKEN"),
    };
    c.output().expect("run blitz")
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).expect("UTF-8 output")
}

#[test]
fn control_pane_list_through_both_copies() {
    let pipe = blitz::control::listen(|line| match parse(&line) {
        Ok((t, Op::List)) if t == TOKEN => concat!(
            r#"{"ok":true,"panes":[{"pane":7,"parent":3,"state":"working","exit":null,"#,
            r#""name":"npm\ttest","cwd":"/x"}]}"#
        )
        .into(),
        Ok(_) => error("no pane 9 that this pane opened"),
        Err(why) => error(why),
    })
    .expect("listen");
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("control-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let copy = dir.join(format!("blitz{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_blitz-hook"), &copy).expect("copy blitz-hook");

    for exe in [copy.as_path(), Path::new(env!("CARGO_BIN_EXE_blitz"))] {
        let out = run(exe, &["pane", "list"], &pipe, Some(TOKEN));
        let err = text(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{exe:?}: {err}");
        assert_eq!(
            text(&out.stdout),
            "7\t3\tworking\tnpm test\t/x\n",
            "{exe:?}"
        );

        let out = run(exe, &["pane", "close", "9"], &pipe, Some(TOKEN));
        assert_eq!(out.status.code(), Some(1), "{exe:?}");
        assert_eq!(
            text(&out.stderr).trim_end(),
            "blitz: no pane 9 that this pane opened"
        );

        let out = run(exe, &["pane", "frob"], &pipe, Some(TOKEN));
        assert_eq!(out.status.code(), Some(2), "{exe:?}");
        assert!(
            text(&out.stderr).starts_with("usage: blitz pane"),
            "{exe:?}"
        );

        // Claude Code can be set to scrub the token from what it runs.
        let out = run(exe, &["pane", "list"], &pipe, None);
        assert_eq!(out.status.code(), Some(1), "{exe:?}");
        assert!(
            text(&out.stderr).contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"),
            "{exe:?}"
        );
    }

    // By this name blitz-hook runs nothing else.
    let out = run(&copy, &["--version"], &pipe, Some(TOKEN));
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        text(&out.stderr).trim_end(),
        "blitz: this copy only runs \"blitz pane\""
    );
    // Never a pipe on another host, which opening would sign in to.
    #[cfg(windows)]
    {
        let unc = r"\\server\pipe\blitz-0123456789abcdef0123456789abcdef";
        let out = run(&copy, &["pane", "list"], unc, Some(TOKEN));
        assert_eq!(out.status.code(), Some(1));
        assert!(text(&out.stderr).contains("not one blitz made"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
