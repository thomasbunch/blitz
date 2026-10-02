//! Opening a folder in the blitz that is already running: a second
//! `blitz --cwd X` sends X to the main window and exits, and that window
//! opens a tab there.
//!
//! Any process on the desktop at the same or a higher integrity level can
//! send this message, so it only ever carries a folder, never a command.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, WM_COPYDATA,
};
use windows::core::HSTRING;
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

/// The payload for `dir`: its path as UTF-16.
pub fn encode(dir: &Path) -> Vec<u16> {
    dir.as_os_str().encode_wide().collect()
}

/// The folder in a payload, or `None` when it is not one blitz sent.
pub fn decode(data: usize, bytes: &[u8]) -> Option<PathBuf> {
    if data != MAGIC
        || bytes.is_empty()
        || bytes.len() > MAX_BYTES
        || !bytes.len().is_multiple_of(2)
    {
        return None;
    }
    let units: Vec<u16> = (bytes.as_chunks().0.iter())
        .map(|&b| u16::from_le_bytes(b))
        .collect();
    let text = String::from_utf16(&units).ok()?;
    (!text.contains('\0')).then(|| PathBuf::from(text))
}

/// Sends `dir` to the running blitz. True only when it took it; on false
/// the caller opens a window of its own.
pub fn send(dir: &Path) -> bool {
    // The receiver's directory is not ours.
    let Ok(dir) = std::path::absolute(dir) else {
        return false;
    };
    let units = encode(&dir);
    let bytes = units.len() * 2;
    if bytes > MAX_BYTES {
        return false;
    }
    let cds = COPYDATASTRUCT {
        dwData: MAGIC,
        cbData: bytes as u32,
        lpData: units.as_ptr() as *mut c_void,
    };
    // SAFETY: a class name and no window name.
    let Ok(hwnd) = (unsafe { FindWindowW(&HSTRING::from(CLASS), None) }) else {
        return false;
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
    sent.0 != 0 && took == 1
}

/// Makes `hwnd` take hand-offs: each folder it is sent arrives as
/// `UserEvent::OpenHere`.
pub fn install(hwnd: isize, proxy: EventLoopProxy<UserEvent>) {
    // Leaked on purpose: the window lives as long as the process.
    let proxy = Box::into_raw(Box::new(proxy));
    // SAFETY: a live window owned by this thread; the subclass proc only
    // reads `proxy`, which is never freed.
    let ok = unsafe { SetWindowSubclass(HWND(hwnd as *mut c_void), Some(proc), 1, proxy as usize) };
    if !ok.as_bool() {
        eprintln!("blitz: cannot take folders from other blitz launches");
    }
}

unsafe extern "system" fn proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    proxy: usize,
) -> LRESULT {
    if msg == WM_COPYDATA {
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
            .filter(|d| d.is_dir())
            .is_some_and(|d| {
                // SAFETY: `install` leaked this proxy for the window's lifetime.
                let proxy = unsafe { &*(proxy as *const EventLoopProxy<UserEvent>) };
                proxy.send_event(UserEvent::OpenHere(d)).is_ok()
            });
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
        assert_eq!(decode(MAGIC, &bytes(&encode(dir))).as_deref(), Some(dir));
    }

    #[test]
    fn handoff_rejects_what_blitz_never_sends() {
        let ok = bytes(&encode(Path::new(r"C:\x")));
        assert_eq!(decode(MAGIC + 1, &ok), None, "another magic");
        assert_eq!(decode(MAGIC, &[]), None, "empty");
        assert_eq!(decode(MAGIC, &ok[..ok.len() - 1]), None, "odd byte count");
        assert_eq!(
            decode(MAGIC, &bytes(&[0x43, 0xd800])),
            None,
            "lone surrogate"
        );
        assert_eq!(decode(MAGIC, &bytes(&[0x43, 0, 0x44])), None, "a NUL");
        let long = bytes(&vec![u16::from(b'a'); MAX_BYTES / 2 + 1]);
        assert_eq!(decode(MAGIC, &long), None, "too long");
        let most = bytes(&vec![u16::from(b'a'); MAX_BYTES / 2]);
        assert!(decode(MAGIC, &most).is_some(), "the longest allowed");
    }
}
