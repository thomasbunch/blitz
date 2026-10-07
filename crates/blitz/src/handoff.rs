//! Launching blitz while it runs: a second `blitz` brings the main window
//! to the front and exits, and a second `blitz --cwd X` also sends X, and
//! that window opens a tab there.
//!
//! Any process on the desktop at the same or a higher integrity level can
//! send this message, so it only ever carries a folder, never a command.
//!
//! The same window procedure sees the window messages winit does not pass
//! on.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, HTCLIENT, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, WM_COPYDATA, WM_EXITSIZEMOVE, WM_MOUSEACTIVATE,
};
use windows::core::{GUID, HSTRING};
use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;

/// The window class of a window that takes hand-offs. Debug builds use
/// their own, so `cargo run` never sends to the installed blitz.
pub const CLASS: &str = if cfg!(debug_assertions) {
    "blitz.dev"
} else {
    "blitz"
};

/// Tells a blitz folder apart from anything else sent as WM_COPYDATA.
const MAGIC: usize = 0x626c_7a01;
/// The largest payload taken, in bytes: a long path is 32767 UTF-16 units.
const MAX_BYTES: usize = 32 * 1024 * 2;
/// How long a running blitz gets to answer before this one opens its own
/// window.
const TIMEOUT_MS: u32 = 2000;

/// What another launch asks of the main window.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    /// Come to the front.
    Show,
    /// Come to the front and open a tab in this folder.
    Open(PathBuf),
}

/// The payload for `dir`: its path as UTF-16. A launch without a folder
/// sends none, which asks only for [`Ask::Show`].
pub fn encode(dir: &Path) -> Vec<u16> {
    dir.as_os_str().encode_wide().collect()
}

/// What a payload asks, or `None` when it is not one blitz sent or its
/// folder is not a local drive folder. Any program on the desktop can send
/// one, and the window procedure looks at it, so a network or device path
/// would make Windows sign in to that host or open that device for the
/// sender. A blitz refused one opens a window of its own there instead.
pub fn decode(data: usize, bytes: &[u8]) -> Option<Ask> {
    if data != MAGIC || bytes.len() > MAX_BYTES || !bytes.len().is_multiple_of(2) {
        return None;
    }
    if bytes.is_empty() {
        return Some(Ask::Show);
    }
    let units: Vec<u16> = (bytes.as_chunks().0.iter())
        .map(|&b| u16::from_le_bytes(b))
        .collect();
    let text = String::from_utf16(&units).ok()?;
    // The same rule as a folder a program reports with OSC 7.
    vt::osc::local_dir(&text).then(|| Ask::Open(PathBuf::from(text)))
}

/// Sends this launch, and `dir` when it names one, to the running blitz.
/// `None` when blitz is not running, else whether it took the launch.
pub fn send(dir: Option<&Path>) -> Option<bool> {
    // SAFETY: a class name and no window name.
    let hwnd = unsafe { FindWindowW(&HSTRING::from(CLASS), None) }.ok()?;
    // The receiver's directory is not ours.
    let units = match dir.map(std::path::absolute) {
        Some(Ok(dir)) => encode(&dir),
        Some(Err(_)) => return Some(false),
        None => Vec::new(),
    };
    let bytes = units.len() * 2;
    if bytes > MAX_BYTES {
        return Some(false);
    }
    let cds = COPYDATASTRUCT {
        dwData: MAGIC,
        cbData: bytes as u32,
        lpData: units.as_ptr() as *mut c_void,
    };
    let mut pid = 0;
    // SAFETY: a window handle and a u32 to fill; a stale handle only fails.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    // Lets it come to the front: the shell gave this process that right
    // when it started it.
    // SAFETY: a plain call.
    let _ = unsafe { AllowSetForegroundWindow(pid) };
    let mut took = 0;
    // SAFETY: `cds` and the units it points to outlive the call, which
    // returns only after the receiver is done with them.
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&raw const cds as isize),
            SMTO_ABORTIFHUNG,
            TIMEOUT_MS,
            Some(&mut took),
        )
    };
    Some(sent.0 != 0 && took == 1)
}

/// Moves `hwnd` to the virtual desktop the user is on, so a launch from
/// another desktop brings blitz over rather than switching desktops. That
/// desktop is known by a window on it: the one in front, or failing that
/// (the taskbar and the Start menu are on every desktop) the next one
/// down that is. A launch from the desktop itself, which is behind every
/// window, looks again from the top.
pub fn to_current_desktop(hwnd: HWND) {
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
    use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
    use windows::Win32::UI::WindowsAndMessaging::{
        GW_HWNDNEXT, GetForegroundWindow, GetTopWindow, GetWindow, IsWindowVisible,
    };
    // SAFETY: COM is set up on this thread, which winit's window lives on;
    // the calls take window handles, and a stale one only fails.
    unsafe {
        let Ok(desktops) =
            CoCreateInstance::<_, IVirtualDesktopManager>(&VirtualDesktopManager, None, CLSCTX_ALL)
        else {
            return;
        };
        if desktops
            .IsWindowOnCurrentVirtualDesktop(hwnd)
            .is_ok_and(|b| b.as_bool())
        {
            return;
        }
        let next = |w| GetWindow(w, GW_HWNDNEXT).unwrap_or_default();
        let shown = |w| w != hwnd && IsWindowVisible(w).as_bool();
        let tops = [
            GetForegroundWindow(),
            GetTopWindow(None).unwrap_or_default(),
        ];
        let windows = (tops.into_iter())
            .flat_map(|w| down_from(w, next, shown))
            .map(|w| {
                let here = desktops.IsWindowOnCurrentVirtualDesktop(w);
                let id = desktops.GetWindowDesktopId(w).unwrap_or_default();
                (here.is_ok_and(|b| b.as_bool()), id)
            });
        if let Some(id) = current_desktop(windows) {
            let _ = desktops.MoveWindowToDesktop(hwnd, &id);
        }
    }
}

/// The first 64 windows `shown` keeps, down the z-order from `w` by
/// `next`. It looks at 1024 at most: the z-order changes while it is
/// walked, and any program can make hidden windows as fast as it likes.
fn down_from(mut w: HWND, next: impl Fn(HWND) -> HWND, shown: impl Fn(HWND) -> bool) -> Vec<HWND> {
    let mut out = Vec::new();
    for _ in 0..1024 {
        if w.is_invalid() || out.len() == 64 {
            break;
        }
        if shown(w) {
            out.push(w);
        }
        w = next(w);
    }
    out
}

/// The current virtual desktop, from windows in front-to-back order, each
/// with whether it shows on the current desktop and the desktop it is on:
/// none for one on every desktop.
fn current_desktop(windows: impl Iterator<Item = (bool, GUID)>) -> Option<GUID> {
    windows
        .filter(|&(here, id)| here && id != GUID::zeroed())
        .map(|(_, id)| id)
        .next()
}

/// A press of a mouse button on the window while it was in the
/// background: the click that brings blitz to the front.
static ACTIVATING: AtomicBool = AtomicBool::new(false);

/// Whether `msg` says the press that comes next is a click on the
/// terminal that brings the window to the front. A click on the title bar
/// or a border has no press after it.
fn activating(msg: u32, lparam: LPARAM) -> bool {
    msg == WM_MOUSEACTIVATE && (lparam.0 & 0xffff) as u32 == HTCLIENT
}

/// Whether the press being handled is the click that brought the window
/// to the front. Asked once per press, so it is true for that one only.
pub fn take_activating_click() -> bool {
    ACTIVATING.swap(false, Ordering::Relaxed)
}

/// Forgets a click that brought the window to the front: it went to the
/// background again before the press came.
pub fn forget_activating_click() {
    ACTIVATING.store(false, Ordering::Relaxed);
}

/// What the window procedure is given.
struct Sub {
    proxy: EventLoopProxy<UserEvent>,
    /// The main window: it takes launches.
    launches: bool,
}

/// Watches `hwnd` for the messages winit does not pass on. With
/// `launches`, it takes hand-offs too: each launch it is sent arrives as
/// `UserEvent::Handoff`.
pub fn install(hwnd: isize, proxy: EventLoopProxy<UserEvent>, launches: bool) {
    // Leaked on purpose: the window lives as long as the process.
    let sub = Box::into_raw(Box::new(Sub { proxy, launches }));
    // SAFETY: a live window owned by this thread; the subclass proc only
    // reads `sub`, which is never freed.
    let ok = unsafe { SetWindowSubclass(HWND(hwnd as *mut c_void), Some(proc), 1, sub as usize) };
    if !ok.as_bool() {
        eprintln!("blitz: cannot watch the window's messages");
    }
}

unsafe extern "system" fn proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    sub: usize,
) -> LRESULT {
    // SAFETY: `install` leaked this for the window's lifetime.
    let sub = unsafe { &*(sub as *const Sub) };
    if activating(msg, lparam) {
        ACTIVATING.store(true, Ordering::Relaxed);
    }
    if msg == WM_EXITSIZEMOVE {
        let _ = sub.proxy.send_event(UserEvent::Sized);
    }
    if msg == WM_COPYDATA && sub.launches {
        // SAFETY: WM_COPYDATA carries a COPYDATASTRUCT; the system copied
        // it and its data into this process, valid until this returns.
        let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
        let bytes = match cds.lpData.is_null() {
            true => &[][..],
            // SAFETY: as above.
            false => unsafe {
                std::slice::from_raw_parts(cds.lpData.cast::<u8>(), cds.cbData as usize)
            },
        };
        let took = decode(cds.dwData, bytes)
            .filter(|a| !matches!(a, Ask::Open(d) if !d.is_dir()))
            .is_some_and(|a| sub.proxy.send_event(UserEvent::Handoff(a)).is_ok());
        return LRESULT(took.into());
    }
    // SAFETY: passes the message on unchanged.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn handoff_round_trips_a_folder() {
        let dir = Path::new(r"C:\Users\jo\Desktop\café ünï 文件");
        let ask = decode(MAGIC, &bytes(&encode(dir)));
        assert_eq!(ask, Some(Ask::Open(dir.into())));
    }

    /// Only a press on the terminal follows; a click on the title bar or a
    /// border brings no press that could be swallowed.
    #[test]
    fn handoff_only_a_click_on_the_terminal_is_kept_from_it() {
        const HTCAPTION: u32 = 2;
        let click = |hit: u32| LPARAM((0x0201 << 16 | hit) as isize);
        assert!(activating(WM_MOUSEACTIVATE, click(HTCLIENT)));
        assert!(!activating(WM_MOUSEACTIVATE, click(HTCAPTION)));
        assert!(!activating(WM_COPYDATA, click(HTCLIENT)));
        ACTIVATING.store(true, Ordering::Relaxed);
        assert!(take_activating_click());
        assert!(!take_activating_click(), "only the first press");
        ACTIVATING.store(true, Ordering::Relaxed);
        forget_activating_click();
        assert!(!take_activating_click());
    }

    /// A launch from another virtual desktop brings blitz to that one,
    /// which the taskbar and other windows on every desktop do not name.
    #[test]
    fn handoff_finds_the_desktop_the_user_is_on() {
        let (a, b) = (GUID::from_u128(1), GUID::from_u128(2));
        let everywhere = (true, GUID::zeroed());
        assert_eq!(current_desktop([(true, a)].into_iter()), Some(a));
        assert_eq!(
            current_desktop([everywhere, (false, b), (true, a)].into_iter()),
            Some(a)
        );
        assert_eq!(current_desktop([everywhere].into_iter()), None);
        assert_eq!(current_desktop(std::iter::empty()), None);
    }

    /// A z-order that never ends, as one a program keeps adding hidden
    /// windows to, is walked only so far.
    #[test]
    fn handoff_walks_the_windows_only_so_far() {
        let h = |n: usize| HWND(n as *mut c_void);
        let next = |w: HWND| h(w.0 as usize + 1);
        assert!(down_from(h(1), next, |_| false).is_empty(), "all hidden");
        let every_other = down_from(h(1), next, |w| (w.0 as usize).is_multiple_of(2));
        assert_eq!(every_other.len(), 64);
        assert_eq!(every_other[0], h(2));
        let few = down_from(h(1), next, |w| (w.0 as usize).is_multiple_of(100));
        assert_eq!(few, (1..=10).map(|i| h(i * 100)).collect::<Vec<_>>());
        assert!(down_from(h(0), next, |_| true).is_empty(), "none");
    }

    /// A second launch without a folder only brings blitz to the front.
    #[test]
    fn handoff_without_a_folder_asks_to_show() {
        assert_eq!(decode(MAGIC, &[]), Some(Ask::Show));
        assert_eq!(decode(MAGIC + 1, &[]), None, "another magic");
    }

    #[test]
    fn handoff_rejects_what_blitz_never_sends() {
        let ok = bytes(&encode(Path::new(r"C:\x")));
        assert_eq!(decode(MAGIC + 1, &ok), None, "another magic");
        assert_eq!(decode(MAGIC, &ok[..ok.len() - 1]), None, "odd byte count");
        let drive: Vec<u16> = r"C:\".encode_utf16().collect();
        assert_eq!(
            decode(MAGIC, &bytes(&[&drive[..], &[0xd800]].concat())),
            None,
            "lone surrogate"
        );
        assert_eq!(
            decode(MAGIC, &bytes(&[&drive[..], &[0x43, 0, 0x44]].concat())),
            None,
            "a NUL"
        );
        let long = bytes(&vec![u16::from(b'a'); MAX_BYTES / 2 + 1]);
        assert_eq!(decode(MAGIC, &long), None, "too long");
        // A folder is held to OSC 7's 4096 bytes; the sender opens a
        // window of its own for a longer one.
        let path = |n: usize| bytes(&encode(Path::new(&format!(r"C:\{}", "a".repeat(n - 3)))));
        assert!(decode(MAGIC, &path(4096)).is_some(), "the longest allowed");
        assert_eq!(decode(MAGIC, &path(4097)), None, "one byte more");
    }

    /// blitz sends absolute paths. A relative one would be read against
    /// the receiver's directory, which is not the sender's.
    #[test]
    fn handoff_takes_only_local_drive_folders() {
        let take =
            |p: &str| decode(MAGIC, &bytes(&encode(Path::new(p)))) == Some(Ask::Open(p.into()));
        for ok in [r"C:\x", r"z:\", "C:/x"] {
            assert!(take(ok), "{ok}");
        }
        for bad in [
            r"x",
            r"x\y",
            r"..\x",
            r"C:x",
            r"\x",
            // Looking at these signs in to a host or opens a device.
            r"\\server\share\x",
            r"\\?\UNC\server\share",
            r"\\?\C:\x",
            r"\\.\pipe\x",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1",
            "C:\\x\ny",
        ] {
            assert!(!take(bad), "{bad}");
        }
    }
}
