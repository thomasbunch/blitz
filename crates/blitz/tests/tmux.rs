//! The `tmux` blitz puts on every pane's PATH, a copy of blitz-hook,
//! against a stand-in for blitz that says back what it was sent.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use blitz::control::{Op, error, parse};
use blitz::tmux::Reply;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn run(exe: &Path, args: &[&str], pipe: Option<&str>, token: Option<&str>) -> Output {
    let mut c = Command::new(exe);
    c.args(args).env("TMUX_TEST_MARK", "1").stdin(Stdio::null());
    match pipe {
        Some(p) => c.env("BLITZ_PIPE", p),
        None => c.env_remove("BLITZ_PIPE"),
    };
    match token {
        Some(t) => c.env("BLITZ_PANE_TOKEN", t),
        None => c.env_remove("BLITZ_PANE_TOKEN"),
    };
    c.output().expect("run tmux")
}

fn text(b: &[u8]) -> String {
    String::from_utf8(b.to_vec()).expect("UTF-8 output")
}

#[test]
fn tmux_answers_its_version_and_forwards_the_rest() {
    let pipe = blitz::control::listen(|line| match parse(&line) {
        Ok((t, Op::Tmux { argv, env })) if t == TOKEN => {
            let marked = env.iter().any(|(k, v)| k == "TMUX_TEST_MARK" && v == "1");
            Reply {
                code: 3,
                out: format!("{} env={marked}\n", argv.join(" ")),
                err: "e\n".into(),
            }
            .json()
        }
        Ok(_) => error("not tmux"),
        Err(why) => error(why),
    })
    .expect("listen");
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tmux-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let tmux = dir.join(format!("tmux{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_blitz-hook"), &tmux).expect("copy blitz-hook");

    // How Claude Code looks for tmux, answered without asking blitz.
    let out = run(&tmux, &["-V"], Some(&pipe), Some(TOKEN));
    assert_eq!(
        (out.status.code(), text(&out.stdout)),
        (Some(0), "tmux 3.4\n".into())
    );
    let out = run(&tmux, &["-V"], Some(&pipe), None);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"));
    let out = run(&tmux, &["-V"], None, Some(TOKEN));
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("BLITZ_PIPE"));

    // The rest goes to blitz, which says what it prints and exits with.
    // Only a teammate's start takes the variables along.
    let list = [
        "-L",
        "claude-swarm-1",
        "list-panes",
        "-t",
        "claude-swarm",
        "-F",
        "#{pane_id}",
    ];
    let out = run(&tmux, &list, Some(&pipe), Some(TOKEN));
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(text(&out.stdout), format!("{} env=false\n", list.join(" ")));
    assert_eq!(text(&out.stderr), "e\n");
    let start = [
        "-L",
        "claude-swarm-1",
        "respawn-pane",
        "-k",
        "-t",
        "%2",
        "--",
        "cd /x && env A=1 /x/claude",
    ];
    let out = run(&tmux, &start, Some(&pipe), Some(TOKEN));
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(text(&out.stdout), format!("{} env=true\n", start.join(" ")));
    let _ = std::fs::remove_dir_all(&dir);
}
