//! The session and its saved output on disk. The only test in this binary:
//! it points `LOCALAPPDATA` at a folder of its own, and no other thread may
//! read the environment meanwhile.

use std::path::{Path, PathBuf};

use blitz::session::{self, State};

const A: &str = "0123456789abcdef0123456789abcdef";
const B: &str = "fedcba9876543210fedcba9876543210";

fn state(cwd: &str) -> State {
    session::from_json(&format!(
        "{{\"v\":1,\"window\":{{\"x\":0,\"y\":0,\"w\":800,\"h\":600,\"maximized\":false}},\
         \"sidebar_expanded\":true,\"active\":0,\"tabs\":[{{\"name\":\"t\",\"focus\":0,\
         \"zoom\":null,\"root\":{{\"pane\":{{\"cwd\":\"{cwd}\",\"claude\":null,\
         \"key\":\"{A}\"}}}}}}]}}"
    ))
    .expect("a valid session")
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .expect("state folder")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn session_files_round_trip() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("session-io-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    // SAFETY: the only test in this binary, so nothing else reads the
    // environment while it changes.
    unsafe { std::env::set_var("LOCALAPPDATA", &root) };
    let dir = session::dir().expect("state folder");
    assert_eq!(dir.parent(), Some(root.as_path()));

    // Nothing saved yet.
    assert_eq!(session::load(), None);
    assert_eq!(session::load_output(A), None);

    // Another window's temporary file is not this one's business while
    // that window may be writing it, but one a crash left goes, on the
    // first save of a run.
    std::fs::create_dir_all(&dir).expect("state folder");
    let (fresh, stale) = (
        dir.join("session.json.4294967295.tmp"),
        dir.join("session.json.4294967294.tmp"),
    );
    std::fs::write(&fresh, "{").expect("write");
    std::fs::write(&stale, "{").expect("write");
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    (std::fs::File::options().write(true).open(&stale))
        .and_then(|f| f.set_modified(hour_ago))
        .expect("age it");

    let (one, two) = (state("one"), state("two"));
    session::save(&one).expect("save");
    assert!(fresh.exists(), "a fresh one stays");
    assert!(!stale.exists(), "a stale one goes");
    std::fs::remove_file(&fresh).expect("remove");
    assert_eq!(session::load(), Some(one.clone()), "with the pane's key");
    // Over an existing file, leaving no temporary one behind.
    session::save(&two).expect("save again");
    assert_eq!(session::load(), Some(two.clone()));
    assert_eq!(names(&dir), ["session.json"]);

    // Output of panes that closed since the last save does not come back.
    let out = |v: &[(&str, &str)]| {
        let v: Vec<_> = v
            .iter()
            .map(|&(k, s)| (k.to_owned(), s.to_owned()))
            .collect();
        session::save_output(&v).expect("save output");
    };
    out(&[(A, "a"), (B, "b")]);
    assert_eq!(session::load_output(B).as_deref(), Some("b"));
    out(&[(A, "c")]);
    assert_eq!(session::load_output(A).as_deref(), Some("c"));
    assert_eq!(session::load_output(B), None);

    // Output is filed by its pane's own key, so saving another layout keeps
    // it: it can only come back into the pane it came from, so a start after
    // a crash or a shutdown still shows it.
    session::save(&one).expect("save");
    assert_eq!(session::load_output(A).as_deref(), Some("c"));
    // Nothing else in the folder is ever read.
    std::fs::write(dir.join("scrollback").join("x.txt"), "x").expect("write");
    for k in ["x", "", r"..\session.json", "../scrollback/x"] {
        assert_eq!(session::load_output(k), None, "{k:?}");
    }

    // A file torn or edited by hand.
    let file = dir.join("session.json");
    let good = std::fs::read_to_string(&file).expect("read");
    std::fs::write(&file, format!("\u{feff}{good}")).expect("write");
    assert_eq!(session::load(), Some(one.clone()), "a byte order mark");
    std::fs::write(&file, &good[..good.len() / 2]).expect("write");
    assert_eq!(session::load(), None, "torn");
    std::fs::write(&file, b"\xff\xfe{").expect("write");
    assert_eq!(session::load(), None, "not UTF-8");

    // The next save puts a good one back.
    session::save(&two).expect("save");
    assert_eq!(session::load(), Some(two.clone()));

    // A session saved before panes had keys filed their output by tab and
    // leaf, which the first start after an update still reads.
    let before_keys = "{\"v\":1,\"window\":{\"x\":0,\"y\":0,\"w\":800,\"h\":600,\
        \"maximized\":false},\"sidebar_expanded\":true,\"active\":0,\"tabs\":[{\"name\":\"t\",\
        \"focus\":0,\"zoom\":null,\"root\":{\"pane\":{\"cwd\":\"old\",\"claude\":null}}}]}";
    std::fs::write(&file, before_keys).expect("write");
    let scrollback = dir.join("scrollback");
    std::fs::create_dir_all(&scrollback).expect("folder");
    std::fs::write(scrollback.join("0-0.txt"), "from before").expect("write");
    let old = session::load().expect("an old session reads");
    assert!(old.layout(1).1.iter().all(|(_, p)| p.key.is_empty()));
    assert_eq!(
        session::load_legacy_output(0, 0).as_deref(),
        Some("from before")
    );
    assert_eq!(session::load_legacy_output(0, 1), None);
    // The next save of output files it by key, so it is read only once.
    out(&[(A, "now")]);
    assert_eq!(session::load_legacy_output(0, 0), None);

    session::clear();
    assert_eq!(session::load(), None);
    assert_eq!(session::load_output(A), None);
    assert!(!dir.join("scrollback").exists());

    // SAFETY: as above.
    unsafe { std::env::remove_var("LOCALAPPDATA") };
    assert_eq!(session::dir(), None);
    assert!(session::save(&one).is_err());
    assert!(session::save_output(&[]).is_err());

    std::fs::remove_dir_all(&root).expect("clean up");
}
