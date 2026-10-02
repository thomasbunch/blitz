//! Clipboard text.

use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// Holds the clipboard open; closes it on drop.
struct Open;

impl Open {
    /// Another program may hold the clipboard for a moment, so try a few
    /// times before giving up.
    fn new(owner: Option<HWND>) -> Option<Open> {
        for _ in 0..10 {
            // SAFETY: a plain Win32 call; a stale window only fails it.
            if unsafe { OpenClipboard(owner) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: the clipboard was opened by this thread.
        let _ = unsafe { CloseClipboard() };
    }
}

/// The clipboard's text, or `None` when it holds no text.
pub fn get_text() -> Option<String> {
    let _open = Open::new(None)?;
    // SAFETY: the clipboard is open; the handle stays valid until it closes,
    // and the locked memory is `GlobalSize` bytes long. Whoever put the text
    // there need not have ended it with a NUL, so the read stops at the end
    // of the block too.
    unsafe {
        let h = GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()?;
        let mem = HGLOBAL(h.0);
        let p = GlobalLock(mem).cast::<u16>();
        if p.is_null() {
            return None;
        }
        let units = std::slice::from_raw_parts(p, GlobalSize(mem) / 2);
        let n = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..n]);
        let _ = GlobalUnlock(mem);
        Some(text)
    }
}

/// Puts `text` on the clipboard. Returns false if that failed.
pub fn set_text(owner: Option<HWND>, text: &str) -> bool {
    let utf16: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let Some(_open) = Open::new(owner) else {
        return false;
    };
    // SAFETY: the clipboard is open; the allocation is large enough for the
    // copy, and the system owns it once SetClipboardData succeeds.
    unsafe {
        if EmptyClipboard().is_err() {
            return false;
        }
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, utf16.len() * 2) else {
            return false;
        };
        let p = GlobalLock(mem).cast::<u16>();
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), p, utf16.len());
        let _ = GlobalUnlock(mem);
        if SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0))).is_err() {
            let _ = GlobalFree(Some(mem));
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Puts `bytes` on the clipboard as text, exactly as given.
    fn set_raw(bytes: &[u8]) -> bool {
        let Some(_open) = Open::new(None) else {
            return false;
        };
        // SAFETY: as in `set_text`.
        unsafe {
            if EmptyClipboard().is_err() {
                return false;
            }
            let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
                return false;
            };
            let p = GlobalLock(mem).cast::<u8>();
            if p.is_null() {
                let _ = GlobalFree(Some(mem));
                return false;
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0))).is_ok()
        }
    }

    /// Writes and reads back the real clipboard, then restores it.
    #[test]
    fn clipboard_round_trip() {
        let before = get_text();
        let text = "blitz clipboard \u{2713}\r\nline 2";
        if !set_text(None, text) {
            eprintln!("skipped: clipboard unavailable");
            return;
        }
        assert_eq!(get_text().as_deref(), Some(text));
        // Another program can leave out the NUL. With an odd size not even
        // the last unit is zero, and the text must still end with the block.
        for n in (3..400).step_by(2) {
            if set_raw(&b"A\0".repeat(n)[..n]) {
                let got = get_text().unwrap_or_default();
                assert!(got.len() <= n / 2, "{n} bytes read as {got:?}");
            }
        }
        if let Some(b) = before {
            set_text(None, &b);
        }
    }
}
