//! Bringing the user back to a session while blitz is in the background:
//! the taskbar button's flash.

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{FLASHW_TRAY, FLASHWINFO, FlashWindowEx};

/// Flashes the taskbar button of `hwnd` `count` times. The button stays
/// lit after the last one until the window comes to the front, so a few
/// are enough and none goes on for ever.
pub fn flash(hwnd: isize, count: u32) {
    let info = FLASHWINFO {
        cbSize: size_of::<FLASHWINFO>() as u32,
        hwnd: HWND(hwnd as *mut c_void),
        dwFlags: FLASHW_TRAY,
        uCount: count,
        dwTimeout: 0,
    };
    // SAFETY: a filled-in struct that outlives the call; a stale window
    // handle only fails.
    let _ = unsafe { FlashWindowEx(&info) };
}
