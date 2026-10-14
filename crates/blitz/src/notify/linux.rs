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
    a.push(lines.get(1..).unwrap_or_default().join("\n"));
    a
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
    let Ok(mut child) = (Command::new("notify-send").args(args(&lines, silent)))
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
}
