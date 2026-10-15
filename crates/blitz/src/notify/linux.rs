//! Desktop notifications through `notify-send`, where the desktop has it.

use std::process::{Command, Stdio};

use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::layout::PaneId;

/// What a notification leaves to keep while it is up; nothing here, as
/// `notify-send` takes no clicks back.
pub type Toast = ();

/// Nothing to hear from: no taskbar button, and no key from every program.
pub fn install(_hwnd: isize, _proxy: EventLoopProxy<UserEvent>) {}

/// The arguments for `notify-send` that show `lines`, the first as the
/// title, without a sound when `silent`.
fn args(lines: &[String], silent: bool) -> Vec<String> {
    let mut a = vec!["--app-name=blitz".to_owned()];
    if silent {
        a.push("--hint=boolean:suppress-sound:true".into());
    }
    // Nothing after this is an option, whatever the text starts with.
    a.push("--".into());
    a.extend(lines.first().cloned());
    let body: Vec<String> = (lines.get(1..).unwrap_or_default().iter())
        .map(|l| markup_text(l))
        .collect();
    a.push(body.join("\n"));
    a
}

/// `line` as body text: notify-send turns `\NNN` back into any byte after
/// blitz cleaned the line, and the desktop reads the body as markup, where
/// a program's `<a href>` would become a link.
fn markup_text(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for c in line.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Shows a notification saying `lines` about pane `id`. `None` when there
/// is nothing to show it with.
pub fn show(
    _id: PaneId,
    lines: &[&str],
    silent: bool,
    _proxy: &EventLoopProxy<UserEvent>,
) -> Result<Option<Toast>, String> {
    let lines = super::clean_lines(lines);
    if lines.is_empty() {
        return Ok(None);
    }
    let Some(exe) = crate::desktop::tool("notify-send") else {
        return Ok(None);
    };
    let Ok(mut child) = (Command::new(exe).args(args(&lines, silent)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return Ok(None);
    };
    std::thread::spawn(move || child.wait());
    Ok(Some(()))
}

/// `notify-send` cannot take a notification back; it times out by itself.
pub fn untoast(_id: PaneId) {}

pub fn untoast_all() {}

/// No Dock to badge.
pub fn badge(_label: Option<&str>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_send_takes_text_that_looks_like_an_option() {
        let lines = ["--help".to_owned(), "-u critical".into(), "x".into()];
        assert_eq!(
            args(&lines, true),
            [
                "--app-name=blitz",
                "--hint=boolean:suppress-sound:true",
                "--",
                "--help",
                "-u critical\nx"
            ]
        );
    }

    #[test]
    fn notify_send_body_stays_text() {
        let lines = [
            "<b>title</b>".to_owned(),
            r"ok \342\200\256gpj.exe \033[31m".into(),
            r#"<a href="https://evil.example">Approve</a> & more"#.into(),
        ];
        assert_eq!(
            args(&lines, false)[1..],
            [
                "--",
                "<b>title</b>",
                "ok \\\\342\\\\200\\\\256gpj.exe \\\\033[31m\n\
                 &lt;a href=\"https://evil.example\"&gt;Approve&lt;/a&gt; &amp; more"
            ]
        );
    }
}
