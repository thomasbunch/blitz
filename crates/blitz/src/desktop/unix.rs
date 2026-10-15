//! macOS and Linux. Most of what Windows reports, as the pointer hiding
//! while typing, has no setting to read here, so these give the usual.

use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(not(target_os = "macos"))]
use std::process::Child;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// The program `name` from a folder on PATH, by its full path. A bare
/// name would also be looked for in an empty or relative PATH entry, that
/// is, in whatever folder blitz was started in, where a planted program
/// would run.
pub fn tool(name: &str) -> Option<PathBuf> {
    tool_in(std::env::var_os("PATH").as_deref(), name)
}

fn tool_in(path: Option<&std::ffi::OsStr>, name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = path.unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".as_ref());
    (std::env::split_paths(path))
        .filter(|d| d.is_absolute())
        .map(|d| d.join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// What `cmd` prints, when it ends well within `deadline` having printed
/// at most `cap` bytes. One that hangs or never stops is killed: what
/// another program serves, such as the clipboard, is not to freeze or
/// fill blitz.
pub fn output_within(cmd: &mut Command, deadline: Duration, cap: u64) -> Option<Vec<u8>> {
    let mut child = (cmd.stdin(Stdio::null()))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let read = out.take(cap + 1).read_to_end(&mut bytes);
        let _ = tx.send(read.map(|_| bytes));
    });
    let got = rx.recv_timeout(deadline);
    let fits = matches!(&got, Ok(Ok(b)) if b.len() as u64 <= cap);
    if !fits {
        let _ = child.kill();
    }
    let ok = child.wait().is_ok_and(|s| s.success());
    match got {
        Ok(Ok(bytes)) if fits && ok => Some(bytes),
        _ => None,
    }
}

/// What the OS is called in an issue report.
#[cfg(target_os = "macos")]
pub const OS: &str = "macOS";
#[cfg(not(target_os = "macos"))]
pub const OS: &str = "Linux";

/// Whether the pointer hides while typing. macOS's own terminal hides it.
pub fn mouse_vanish() -> bool {
    cfg!(target_os = "macos")
}

/// Whether the system shows animations: on macOS, unless Reduce motion is
/// on in Accessibility, Display.
pub fn animations_on() -> bool {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: plain queries of the shared workspace.
        unsafe {
            !objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
        }
    }
    #[cfg(not(target_os = "macos"))]
    true
}

/// How long ago the last key or mouse input anywhere was; zero when the
/// system cannot say.
pub fn idle_for() -> Duration {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGEventSourceSecondsSinceLastEventType(state: i32, event: u32) -> f64;
        }
        // kCGEventSourceStateCombinedSessionState and kCGAnyInputEventType.
        // SAFETY: a plain query with no pointers.
        let s = unsafe { CGEventSourceSecondsSinceLastEventType(0, !0) };
        Duration::try_from_secs_f64(s).unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    Duration::ZERO
}

/// Lines to scroll for each wheel notch.
pub fn scroll_lines() -> u32 {
    3
}

/// The OS version, such as `macOS 15.1 (Build 24B83)` or the
/// distribution's name.
pub fn os_version() -> String {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: plain queries of this process's info.
        let v = unsafe {
            objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersionString()
        };
        format!("macOS {v}")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        os_name(&release).unwrap_or_else(|| "unknown".into())
    }
}

/// The `PRETTY_NAME` of an `os-release` file.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn os_name(release: &str) -> Option<String> {
    let line = release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))?;
    Some(line.trim().trim_matches('"').to_owned()).filter(|n| !n.is_empty())
}

/// Windows starts blitz again after it restarts for an update; nothing
/// here asks for that.
pub fn restart_after_reboot() {}

/// Whether blitz starts when the user signs in. Setup does not offer it
/// here yet.
pub fn starts_at_sign_in() -> bool {
    false
}

/// Windows' screen readers follow the system caret; nothing here does.
pub fn place_caret(
    _hwnd: isize,
    _at: (i32, i32),
    size: (u32, u32),
    _made: Option<(u32, u32)>,
) -> Option<(u32, u32)> {
    Some(size)
}

pub fn drop_caret() {}

/// The local time as `2026-10-02 14:32`.
pub fn local_stamp() -> String {
    // SAFETY: `time` and `tm` are plain values the calls fill.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        format!(
            "{}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min
        )
    }
}

/// Whether blitz runs as root.
pub fn elevated() -> bool {
    // SAFETY: a plain query.
    unsafe { libc::geteuid() == 0 }
}

/// Folders nobody means to work in that blitz starts in when opened
/// from the Finder or the Dock: the root folder.
pub fn not_a_start() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

/// Calls `changed` whenever a file in `dir`, or a folder under it, is
/// written, until it returns false. It looks every second.
pub fn watch_dir(dir: PathBuf, mut changed: impl FnMut() -> bool + Send + 'static) {
    std::thread::spawn(move || {
        let mut was = stamp(&dir);
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = stamp(&dir);
            if now != was {
                was = now;
                if !changed() {
                    return;
                }
            }
        }
    });
}

/// The names, sizes and times of the files in `dir` and the folders in
/// it, which a write changes.
fn stamp(dir: &Path) -> Vec<(PathBuf, u64, Option<SystemTime>)> {
    let mut out = Vec::new();
    let mut dirs = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else {
                continue;
            };
            if m.is_dir() && depth < 2 {
                dirs.push((e.path(), depth + 1));
            }
            out.push((e.path(), m.len(), m.modified().ok()));
        }
    }
    out.sort();
    out
}

/// How long two clicks may be apart to count as a double click.
pub fn double_click() -> Duration {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: a plain query.
        let s = unsafe { objc2_app_kit::NSEvent::doubleClickInterval() };
        Duration::try_from_secs_f64(s).unwrap_or(Duration::from_millis(500))
    }
    #[cfg(not(target_os = "macos"))]
    Duration::from_millis(400)
}

/// The default sound for something that wants the user.
pub fn beep() {
    #[cfg(target_os = "macos")]
    // SAFETY: a plain call.
    unsafe {
        objc2_app_kit::NSBeep();
    }
}

/// Says why blitz could not start. It already went to the console.
pub fn error_box(_text: &str) {}

/// winit brings the window to the front; see `App::to_front`.
pub fn to_front(_hwnd: isize) {}

/// Where the pointer is now; the app goes by where it last saw it.
pub fn pointer(_hwnd: isize) -> Option<(f64, f64)> {
    None
}

/// The frame follows the theme through winit; see `App::frame_theme`.
pub fn frame_theme(_hwnd: isize, _light: bool, _caption: u32, _text: u32, _border: u32) {}

/// The Dock icon on macOS. Progress has no place there.
#[derive(Default)]
pub struct Taskbar;

impl Taskbar {
    pub fn progress(&mut self, _hwnd: isize, _p: Option<crate::render::chrome::Progress>) -> bool {
        true
    }

    /// Badges the Dock icon while a session wants the user; `None`
    /// clears it.
    pub fn badge(&mut self, _hwnd: isize, dot: Option<(u32, u32, bool)>, _label: &str) -> bool {
        crate::notify::badge(dot.map(|(_, _, ring)| if ring { "\u{2713}" } else { "!" }));
        true
    }

    pub fn lost(&mut self) {}
}

/// Keeps the computer from going to sleep by itself while on: through
/// macOS's activity, or on Linux `systemd-inhibit` where there is one. A
/// sleep the user asks for still happens.
#[derive(Default)]
pub struct Awake {
    #[cfg(target_os = "macos")]
    activity: Option<objc2::rc::Retained<objc2_foundation::NSObject>>,
    #[cfg(not(target_os = "macos"))]
    inhibit: Option<Child>,
}

impl Awake {
    /// Turns it `on` or off; false when the system refused. On Linux that
    /// is only when `systemd-inhibit` could not start: one that starts but
    /// finds no logind ends at once, unseen.
    pub fn set(&mut self, on: bool) -> bool {
        #[cfg(target_os = "macos")]
        {
            use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
            let info = NSProcessInfo::processInfo();
            match (on, self.activity.take()) {
                // SAFETY: plain calls on this process's info.
                (true, None) => unsafe {
                    let why = NSString::from_str("A session is working");
                    let options = NSActivityOptions::NSActivityIdleSystemSleepDisabled;
                    self.activity = Some(info.beginActivityWithOptions_reason(options, &why));
                },
                // SAFETY: an activity this process began.
                (false, Some(a)) => unsafe { info.endActivity(&a) },
                (_, a) => self.activity = a,
            }
            true
        }
        #[cfg(not(target_os = "macos"))]
        {
            if !on {
                if let Some(mut c) = self.inhibit.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                return true;
            }
            if self.inhibit.is_none() {
                self.inhibit = inhibit();
            }
            self.inhibit.is_some()
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// A `systemd-inhibit` that holds off idle sleep until it is killed or
/// blitz exits.
#[cfg(not(target_os = "macos"))]
fn inhibit() -> Option<Child> {
    // `cat` waits on a pipe only blitz holds, so the lock ends with blitz
    // however it ends.
    let cat = tool("cat")?;
    Command::new(tool("systemd-inhibit")?)
        .args([
            // Idle sleep only: a suspend the user asks for still happens.
            "--what=idle",
            "--who=blitz",
            "--why=A session is working",
            "--mode=block",
        ])
        .arg(cat)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_distribution_is_named_by_its_pretty_name() {
        let release = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(os_name(release).as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(os_name("ID=arch\n"), None);
        assert_eq!(os_name("PRETTY_NAME=\n"), None);
    }

    #[test]
    fn tools_come_only_from_absolute_folders() {
        assert_eq!(tool_in(Some("".as_ref()), "sh"), None);
        assert_eq!(tool_in(Some(".:bin".as_ref()), "sh"), None);
        assert_eq!(
            tool_in(Some(":/bin".as_ref()), "sh"),
            Some("/bin/sh".into())
        );
        assert_eq!(tool_in(Some("/bin".as_ref()), "no-such-tool"), None);
    }

    #[test]
    fn output_within_stops_a_tool_that_hangs_or_floods() {
        let sh = |script: &str| {
            let mut c = Command::new("/bin/sh");
            c.args(["-c", script]);
            c
        };
        let second = Duration::from_secs(1);
        assert_eq!(
            output_within(&mut sh("printf hi"), second, 2),
            Some(b"hi".to_vec())
        );
        assert_eq!(output_within(&mut sh("printf hi; exit 1"), second, 9), None);
        assert_eq!(output_within(&mut sh("yes"), second * 5, 1 << 16), None);
        let start = std::time::Instant::now();
        assert_eq!(output_within(&mut sh("exec sleep 30"), second, 9), None);
        assert!(start.elapsed() < second * 10);
    }

    #[test]
    fn the_stamp_changes_when_a_file_is_written() {
        let dir = std::env::temp_dir().join(format!("blitz-watch-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        let before = stamp(&dir);
        std::fs::write(dir.join("themes").join("x"), "a").unwrap();
        let after = stamp(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(before, after);
    }

    #[test]
    fn the_time_is_stamped_to_the_minute() {
        let s = local_stamp();
        assert_eq!(s.len(), 16, "{s}");
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], " ");
    }
}
