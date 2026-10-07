//! A panic on the window's thread is written down for the next start. The
//! only test in this binary: it points `LOCALAPPDATA` at a folder of its
//! own and sets the panic hook, which every thread shares.
#![cfg(windows)]

use std::path::PathBuf;

use blitz::session;

#[test]
fn a_panic_on_the_window_thread_is_told_of_once() {
    let root =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("root");
    // SAFETY: the only test in this binary, so nothing else reads the
    // environment while it changes.
    unsafe { std::env::set_var("LOCALAPPDATA", &root) };
    assert_eq!(session::take_crash(), None);

    blitz::app::catch_crashes();
    // Another thread's panic is caught where it happens, as a pane's is.
    let other = std::thread::spawn(|| panic!("a reader panic")).join();
    assert!(other.is_err());
    assert_eq!(session::take_crash(), None);

    let window = std::panic::catch_unwind(|| panic!("a window panic"));
    assert!(window.is_err());
    let file = session::take_crash().expect("a crash file");
    let text = std::fs::read_to_string(&file).expect("read");
    assert!(text.starts_with(&format!("blitz {} ", env!("CARGO_PKG_VERSION"))));
    assert!(
        text.contains("a window panic") && text.contains("crash.rs"),
        "{text}"
    );
    // The next start says nothing, and the file stays to be read.
    assert_eq!(session::take_crash(), None);
    assert!(file.is_file());

    std::fs::remove_dir_all(&root).expect("clean up");
}
