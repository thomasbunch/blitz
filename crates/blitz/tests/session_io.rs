//! The session and its saved output on disk. The only test in this binary:
//! it points `LOCALAPPDATA` at a folder of its own, and no other thread may
//! read the environment meanwhile.

use std::path::{Path, PathBuf};

use blitz::session::{self, State};

fn state(cwd: &str) -> State {
    session::from_json(&format!(
        "{{\"v\":1,\"window\":{{\"x\":0,\"y\":0,\"w\":800,\"h\":600,\"maximized\":false}},\
         \"sidebar_expanded\":true,\"active\":0,\"tabs\":[{{\"name\":\"t\",\"focus\":0,\
         \"zoom\":null,\"root\":{{\"pane\":{{\"cwd\":\"{cwd}\",\"claude\":null}}}}}}]}}"
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
    assert_eq!(session::load_output(0, 0), None);

    let (one, two) = (state("one"), state("two"));
    session::save(&one).expect("save");
    assert_eq!(session::load(), Some(one.clone()));
    // Over an existing file, leaving no temporary one behind.
    session::save(&two).expect("save again");
    assert_eq!(session::load(), Some(two.clone()));
    assert_eq!(names(&dir), ["session.json"]);

    // Output of panes that closed since the last save does not come back.
    let out = |v: &[(usize, usize, &str)]| {
        let v: Vec<_> = v.iter().map(|&(t, l, s)| (t, l, s.to_owned())).collect();
        session::save_output(&v).expect("save output");
    };
    out(&[(0, 0, "a"), (1, 0, "b")]);
    assert_eq!(session::load_output(1, 0).as_deref(), Some("b"));
    out(&[(0, 0, "c")]);
    assert_eq!(session::load_output(0, 0).as_deref(), Some("c"));
    assert_eq!(session::load_output(1, 0), None);

    // Output is filed by tab and leaf of the layout it was saved with, so a
    // new layout drops it: after a crash it would show in the wrong pane.
    session::save(&one).expect("save");
    assert_eq!(session::load_output(0, 0), None);

    // A file torn or edited by hand.
    let file = dir.join("session.json");
    let good = std::fs::read_to_string(&file).expect("read");
    std::fs::write(&file, format!("\u{feff}{good}")).expect("write");
    assert_eq!(session::load(), Some(one.clone()), "a byte order mark");
    std::fs::write(&file, &good[..good.len() / 2]).expect("write");
    assert_eq!(session::load(), None, "torn");
    std::fs::write(&file, b"\xff\xfe{").expect("write");
    assert_eq!(session::load(), None, "not UTF-8");

    // Another window's temporary file is not this one's business.
    let other = dir.join("session.json.4294967295.tmp");
    std::fs::write(&other, "{").expect("write");
    session::save(&two).expect("save");
    assert_eq!(session::load(), Some(two.clone()));
    std::fs::remove_file(&other).expect("remove");

    // Output that cannot be removed keeps the old layout with it rather
    // than pairing it with the new one.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        out(&[(0, 0, "kept")]);
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(dir.join("scrollback").join("0-0.txt"))
            .expect("hold");
        assert!(session::save(&one).is_err());
        drop(held);
        assert_eq!(session::load(), Some(two.clone()));
        assert_eq!(session::load_output(0, 0).as_deref(), Some("kept"));
        assert_eq!(names(&dir), ["scrollback", "session.json"]);
    }

    session::clear();
    assert_eq!(session::load(), None);
    assert_eq!(session::load_output(0, 0), None);
    assert!(!dir.join("scrollback").exists());

    // SAFETY: as above.
    unsafe { std::env::remove_var("LOCALAPPDATA") };
    assert_eq!(session::dir(), None);
    assert!(session::save(&one).is_err());
    assert!(session::save_output(&[]).is_err());

    std::fs::remove_dir_all(&root).expect("clean up");
}
