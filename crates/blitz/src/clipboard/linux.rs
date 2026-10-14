//! The clipboard on Linux, through the desktop's own tools: `wl-copy` and
//! `wl-paste` on Wayland, `xclip` or `xsel` on X11.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// The commands that read the clipboard as `mime`, and that write text
/// to it, for the session blitz runs in, in the order they are tried.
fn readers(mime: &str) -> Vec<Vec<String>> {
    let a = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect();
    let mut tools = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        tools.push(a(&["wl-paste", "--no-newline", "--type", mime]));
    }
    tools.push(a(&["xclip", "-selection", "clipboard", "-o", "-t", mime]));
    if mime.starts_with("text/plain") {
        tools.push(a(&["xsel", "--clipboard", "--output"]));
    }
    tools
}

fn writers() -> Vec<&'static [&'static str]> {
    let mut tools: Vec<&[&str]> = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        tools.push(&["wl-copy"]);
    }
    tools.push(&["xclip", "-selection", "clipboard", "-i"]);
    tools.push(&["xsel", "--clipboard", "--input"]);
    tools
}

/// What the first tool that runs gives, `None` if none did or the
/// clipboard holds nothing of that type.
fn read(mime: &str) -> Option<Vec<u8>> {
    readers(mime).into_iter().find_map(|tool| {
        let out = (Command::new(&tool[0]).args(&tool[1..]))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        out.status.success().then_some(out.stdout)
    })
}

/// The types the clipboard holds.
fn types() -> Vec<String> {
    let listers: [&[&str]; 2] = [
        &["wl-paste", "--list-types"],
        &["xclip", "-selection", "clipboard", "-o", "-t", "TARGETS"],
    ];
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    (listers.iter())
        .filter(|l| wayland || l[0] != "wl-paste")
        .find_map(|l| {
            let out = (Command::new(l[0]).args(&l[1..]))
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            out.status.success().then(|| {
                (String::from_utf8_lossy(&out.stdout).lines())
                    .map(str::to_owned)
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// The clipboard holds an image, such as a screenshot.
pub fn has_image() -> bool {
    types().iter().any(|t| t.starts_with("image/"))
}

/// The clipboard's text, or `None` when it holds no text.
pub fn get_text() -> Option<String> {
    let bytes = read("text/plain;charset=utf-8").or_else(|| read("UTF8_STRING"))?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The files copied in the file manager, or `None` when the clipboard
/// holds none.
pub fn get_files() -> Option<Vec<PathBuf>> {
    let list = String::from_utf8(read("text/uri-list")?).ok()?;
    let files: Vec<PathBuf> = (list.lines())
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| vt::osc::file_url_path(l.trim()))
        .map(PathBuf::from)
        .collect();
    (!files.is_empty()).then_some(files)
}

/// Puts `text` on the clipboard. Returns false if no tool took it. The
/// owner window is for Windows.
pub fn set_text(_owner: Option<isize>, text: &str) -> bool {
    writers().into_iter().any(|tool| {
        let Ok(mut child) = (Command::new(tool[0]).args(&tool[1..]))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            return false;
        };
        let wrote = (child.stdin.take()).is_some_and(|mut i| i.write_all(text.as_bytes()).is_ok());
        // The tools fork to keep serving the clipboard, so this returns.
        child.wait().is_ok_and(|s| s.success()) && wrote
    })
}
